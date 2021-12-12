use futures_util::{SinkExt, StreamExt, TryStreamExt};
use nertsio_types as ni_ty;
use std::sync::{Arc, Mutex};

use crate::{res_to_error, ConnectionState, SharedInfo};

pub enum ConnectionType {
    CreateGame {
        public: bool,
    },
    JoinPublicGame {
        server: ni_ty::protocol::ServerConnectionInfo,
        game_id: u32,
    },
    JoinPrivateGame {
        server_id: u8,
        game_id: u32,
    },
}

#[derive(Debug)]
pub enum ConnectionMessage {
    Game(ni_ty::protocol::GameMessageC2S),
    Leave,
}

impl From<ni_ty::protocol::GameMessageC2S> for ConnectionMessage {
    fn from(src: ni_ty::protocol::GameMessageC2S) -> Self {
        ConnectionMessage::Game(src)
    }
}

pub(crate) async fn handle_connection<
    C: hyper::client::connect::Connect + Clone + Send + Sync + 'static,
>(
    http_client: &hyper::Client<C>,
    connection_type: ConnectionType,
    info_mutex: &std::sync::Mutex<ConnectionState>,
    mut game_msg_recv: tokio::sync::mpsc::UnboundedReceiver<ConnectionMessage>,
    settings_mutex: Arc<Mutex<crate::Settings>>,
) -> Result<(), anyhow::Error> {
    let (server, game_id, new_game_public) = match connection_type {
        ConnectionType::CreateGame { public } => {
            let resp = res_to_error(
                http_client
                    .request(
                        hyper::Request::post(format!(
                            "{}servers:pick_for_new_game",
                            crate::COORDINATOR_URL
                        ))
                        .body(Default::default())
                        .unwrap(),
                    )
                    .await?,
            )
            .await?;

            let resp = hyper::body::to_bytes(resp.into_body()).await?;
            let resp: ni_ty::protocol::ServerConnectionInfo = serde_json::from_slice(&resp)?;

            (resp, None, Some(public))
        }
        ConnectionType::JoinPublicGame { server, game_id } => (server, Some(game_id), None),
        ConnectionType::JoinPrivateGame { server_id, game_id } => {
            let resp = res_to_error(
                http_client
                    .request(
                        hyper::Request::get(format!(
                            "{}servers/{}",
                            crate::COORDINATOR_URL,
                            server_id
                        ))
                        .body(Default::default())
                        .unwrap(),
                    )
                    .await?,
            )
            .await?;

            let resp = hyper::body::to_bytes(resp.into_body()).await?;
            let resp: ni_ty::protocol::ServerConnectionInfo = serde_json::from_slice(&resp)?;

            (resp, Some(game_id), None)
        }
    };

    let host = server.address_ipv4.into();
    let server_id = server.server_id;

    let mut endpoint = quinn::Endpoint::client(
        (
            match host {
                std::net::SocketAddr::V4(_) => {
                    std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED)
                }
                std::net::SocketAddr::V6(_) => {
                    std::net::IpAddr::V6(std::net::Ipv6Addr::UNSPECIFIED)
                }
            },
            0,
        )
            .into(),
    )?;
    endpoint.set_default_client_config(quinn::ClientConfig::new(Arc::new({
        let mut cfg = rustls::ClientConfig::builder()
            .with_safe_defaults()
            .with_root_certificates(rustls::RootCertStore { roots: vec![] })
            .with_no_client_auth();
        cfg.dangerous()
            .set_certificate_verifier(Arc::new(InsecureVerifier));
        cfg
    })));

    let conn = endpoint.connect(host, "nio.invalid")?.await?;

    println!("connected");

    let handshake_stream = conn.connection.open_bi().await?;

    println!("opened stream");

    let mut handshake_stream_send =
        async_bincode::AsyncBincodeWriter::<_, ni_ty::protocol::HandshakeMessageC2S, _>::from(
            handshake_stream.0,
        )
        .for_async();
    let handshake_stream_recv = async_bincode::AsyncBincodeReader::<
        _,
        ni_ty::protocol::HandshakeMessageS2C,
    >::from(handshake_stream.1);

    let hello_msg = ni_ty::protocol::HandshakeMessageC2S::Hello {
        name: (*settings_mutex.lock().unwrap()).name.clone(),
        game_id,
        new_game_public,
        protocol_version: ni_ty::protocol::PROTOCOL_VERSION,
        min_protocol_version: ni_ty::protocol::PROTOCOL_VERSION,
    };
    handshake_stream_send.send(hello_msg).await?;

    println!("sent hello");

    let (first_message, handshake_stream_recv) = handshake_stream_recv.into_future().await;
    let first_message = first_message.ok_or(anyhow::anyhow!("Failed to complete handshake"))??;

    let _ = (handshake_stream_recv, handshake_stream_send);

    #[allow(irrefutable_let_patterns)]
    if let ni_ty::protocol::HandshakeMessageS2C::Hello = first_message {
    } else {
        anyhow::bail!("Unknown handshake response");
    }

    println!("aaa");

    let (game_stream_res, _bi_streams) = conn.bi_streams.into_future().await;
    let game_stream = game_stream_res.ok_or(anyhow::anyhow!("Missing game stream"))??;

    println!("bbb");

    let mut game_stream_send =
        async_bincode::AsyncBincodeWriter::<_, ni_ty::protocol::GameMessageC2S, _>::from(
            game_stream.0,
        )
        .for_async();
    let game_stream_recv =
        async_bincode::AsyncBincodeReader::<_, ni_ty::protocol::GameMessageS2C>::from(
            game_stream.1,
        );

    println!("wat");

    let (send_leave, recv_leave) = tokio::sync::oneshot::channel();

    if let futures_util::future::Either::Left((Err(err), _)) = futures_util::future::select(
        Box::pin(futures_util::future::try_join4(
            async move {
                while let Some(msg) = game_msg_recv.recv().await {
                    match msg {
                        ConnectionMessage::Game(msg) => {
                            println!("sending {:?}", msg);
                            game_stream_send.send(msg).await?;
                        }
                        ConnectionMessage::Leave => {
                            let _ = send_leave.send(()); // if it's dropped we must have already disconnected?
                            break;
                        }
                    }
                }
                Result::<_, anyhow::Error>::Ok(())
            },
            async move {
                let mut interval = tokio::time::interval(std::time::Duration::from_millis(50));
                let mut seq: u32 = 0;

                loop {
                    interval.tick().await;

                    let mut lock = info_mutex.lock().unwrap();
                    let shared = lock.as_info_mut().unwrap();

                    if shared.game.hand.is_some() {
                        if let Some(mouse_pos) = shared.last_mouse_position {
                            conn.connection.send_datagram(
                                bincode::serialize(
                                    &ni_ty::protocol::DatagramMessageC2S::UpdateMouseState {
                                        seq,
                                        state: ni_ty::MouseState {
                                            position: mouse_pos,
                                            held: shared.my_held_state,
                                        },
                                    },
                                )
                                .unwrap()
                                .into(),
                            )?;

                            seq += 1;
                        }
                    }
                }

                // allows inferring return type
                #[allow(unreachable_code)]
                Ok(())
            },
            async {
                conn.datagrams
                    .map_err(Into::into)
                    .try_for_each(|bytes| async move {
                        use ni_ty::protocol::DatagramMessageS2C;

                        let msg: DatagramMessageS2C = bincode::deserialize(&bytes)?;
                        match msg {
                            DatagramMessageS2C::UpdateMouseState {
                                player_idx,
                                seq,
                                state,
                            } => {
                                let mut lock = info_mutex.lock().unwrap();
                                let shared = (*lock).as_info_mut().unwrap();

                                if let Some(hand_mouse_states) = shared.hand_mouse_states.as_mut() {
                                    let mouse_state = &mut hand_mouse_states[player_idx as usize];
                                    if match mouse_state {
                                        Some(state) => state.0 < seq,
                                        None => true,
                                    } {
                                        *mouse_state = Some((seq, state));
                                    }
                                }
                            }
                        }

                        Result::<_, anyhow::Error>::Ok(())
                    })
                    .await?;

                Ok(())
            },
            async move {
                game_stream_recv
                    .map_err(Into::into)
                    .try_for_each(|msg| async move {
                        use ni_ty::protocol::GameMessageS2C;

                        println!("received {:?}", msg);

                        match msg {
                            GameMessageS2C::Joined {
                                info,
                                your_player_id,
                            } => {
                                *info_mutex.lock().unwrap() =
                                    ConnectionState::Connected(SharedInfo {
                                        hand_mouse_states: info
                                            .hand
                                            .as_ref()
                                            .map(|hand| vec![None; hand.players().len()]),
                                        game: info,
                                        my_player_id: your_player_id,
                                        pending_actions: Default::default(),
                                        self_called_nerts: false,
                                        my_held_state: None,
                                        last_mouse_position: None,
                                        server_id,
                                    });
                            }
                            GameMessageS2C::PlayerJoin { id, info } => {
                                (*info_mutex.lock().unwrap())
                                    .as_info_mut()
                                    .unwrap()
                                    .game
                                    .players
                                    .insert(id, info);
                            }
                            GameMessageS2C::PlayerLeave { id } => {
                                (*info_mutex.lock().unwrap())
                                    .as_info_mut()
                                    .unwrap()
                                    .game
                                    .players
                                    .remove(&id);
                            }
                            GameMessageS2C::PlayerUpdateReady { id, value } => {
                                (*info_mutex.lock().unwrap())
                                    .as_info_mut()
                                    .unwrap()
                                    .game
                                    .players
                                    .get_mut(&id)
                                    .unwrap()
                                    .ready = value;
                            }
                            GameMessageS2C::HandStart { info } => {
                                let mut lock = info_mutex.lock().unwrap();
                                let shared = (*lock).as_info_mut().unwrap();

                                shared.hand_mouse_states = Some(vec![None; info.players().len()]);
                                shared.game.hand = Some(info);
                                shared.my_held_state = None;
                            }
                            GameMessageS2C::PlayerHandAction { player, action } => {
                                let mut lock = info_mutex.lock().unwrap();
                                let shared = lock.as_info_mut().unwrap();

                                let hand = shared.game.hand.as_mut().unwrap();

                                if let Some(my_player_idx) = hand
                                    .players()
                                    .iter()
                                    .position(|player| player.player_id() == shared.my_player_id)
                                {
                                    if player == my_player_idx as u8 {
                                        // my move, check if matches expected

                                        while let Some(front) = shared.pending_actions.pop_front() {
                                            if front == action {
                                                break;
                                            }
                                        }
                                    }
                                }

                                hand.apply(player, action).unwrap();
                            }
                            GameMessageS2C::NertsCalled { player: _ } => {
                                let mut lock = info_mutex.lock().unwrap();
                                let shared = lock.as_info_mut().unwrap();

                                let hand = shared.game.hand.as_mut().unwrap();

                                hand.nerts_called = true;
                            }
                            GameMessageS2C::HandEnd { scores } => {
                                let mut lock = info_mutex.lock().unwrap();
                                let shared = lock.as_info_mut().unwrap();

                                let hand_state = shared.game.hand.take().unwrap();

                                for (player, score) in hand_state.players().iter().zip(scores) {
                                    if let Some(info) =
                                        shared.game.players.get_mut(&player.player_id())
                                    {
                                        info.score += score;
                                    }
                                }

                                for player in shared.game.players.values_mut() {
                                    player.ready = false;
                                }
                                shared.self_called_nerts = false;
                                shared.pending_actions.clear();
                            }
                        }

                        Ok(())
                    })
                    .await
            },
        )),
        recv_leave,
    )
    .await
    {
        Err(err)
    } else {
        Ok(())
    }
}

struct InsecureVerifier;
impl rustls::client::ServerCertVerifier for InsecureVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::Certificate,
        _intermediates: &[rustls::Certificate],
        _server_name: &rustls::client::ServerName,
        _scts: &mut dyn Iterator<Item = &[u8]>,
        _ocsp_response: &[u8],
        _now: std::time::SystemTime,
    ) -> Result<rustls::client::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::ServerCertVerified::assertion())
    }
}
