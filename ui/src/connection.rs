#[cfg(target_family = "wasm")]
use bincode::Options;
#[cfg(not(target_family = "wasm"))]
use futures_util::SinkExt;
use futures_util::{StreamExt, TryStreamExt};
use nertsio_types as ni_ty;
use std::sync::{Arc, Mutex};

use crate::{ConnectionState, SharedInfo};

pub enum ConnectionType<'a> {
    CreateGame {
        public: bool,
    },
    JoinPublicGame {
        server: ni_ty::protocol::ServerConnectionInfo<'a>,
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

#[derive(Debug)]
pub struct CloseError {
    code: u16,
}

impl CloseError {
    pub fn code(&self) -> u16 {
        self.code
    }
}

impl std::fmt::Display for CloseError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "WebSocket closed with code {}", self.code())
    }
}

impl std::error::Error for CloseError {}

#[cfg(target_family = "wasm")]
fn bi_bincode_options() -> impl bincode::Options {
    bincode::options()
        .with_limit(u32::max_value() as u64)
        .allow_trailing_bytes()
}

pub(crate) async fn handle_connection(
    http_client: &reqwest::Client,
    connection_type: ConnectionType<'_>,
    info_mutex: &std::sync::Mutex<ConnectionState>,
    mut game_msg_recv: futures_channel::mpsc::UnboundedReceiver<ConnectionMessage>,
    settings_mutex: Arc<Mutex<crate::Settings>>,
) -> Result<(), anyhow::Error> {
    let (server, game_id, new_game_public) = match connection_type {
        ConnectionType::CreateGame { public } => {
            let resp = http_client
                .post(format!(
                    "{}servers:pick_for_new_game?protocol_version={}&min_protocol_version={}",
                    crate::COORDINATOR_URL,
                    ni_ty::protocol::PROTOCOL_VERSION,
                    ni_ty::protocol::PROTOCOL_VERSION,
                ))
                .send()
                .await?
                .error_for_status()?;

            let resp: ni_ty::protocol::ServerConnectionInfo = resp.json().await?;

            (resp, None, Some(public))
        }
        ConnectionType::JoinPublicGame { server, game_id } => (server, Some(game_id), None),
        ConnectionType::JoinPrivateGame { server_id, game_id } => {
            let resp = http_client
                .get(format!("{}servers/{}", crate::COORDINATOR_URL, server_id))
                .send()
                .await?
                .error_for_status()?;

            let resp: ni_ty::protocol::ServerConnectionInfo = resp.json().await?;

            (resp, Some(game_id), None)
        }
    };

    let server_id = server.server_id;

    #[cfg(not(target_family = "wasm"))]
    let conn = {
        let host: std::net::SocketAddr = server.address_ipv4.into();

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

        endpoint.connect(host, "nio.invalid")?.await?
    };

    #[cfg(target_family = "wasm")]
    let mut conn = {
        struct WSDropper(wasm_sockets::EventClient);

        impl std::ops::Deref for WSDropper {
            type Target = wasm_sockets::EventClient;

            fn deref(&self) -> &Self::Target {
                &self.0
            }
        }

        impl std::ops::DerefMut for WSDropper {
            fn deref_mut(&mut self) -> &mut <Self as std::ops::Deref>::Target {
                &mut self.0
            }
        }

        impl Drop for WSDropper {
            fn drop(&mut self) {
                log::debug!("closing WS");
                if let Err(err) = self.0.close() {
                    log::error!("Failed to close socket: {:?}", err);
                }
            }
        }

        let mut conn = WSDropper(wasm_sockets::EventClient::new(&match server.hostname {
            Some(hostname) => format!("wss://{}:{}", hostname, server.address_ipv4.port()),
            None => format!("ws://{}", server.address_ipv4),
        })?);

        let (connect_send, mut connect_recv) = futures_channel::mpsc::channel(1);
        let connect_send = std::cell::RefCell::new(connect_send);

        conn.set_on_connection(Some(Box::new(move |_| {
            connect_send.borrow_mut().try_send(()).unwrap();
        })));

        connect_recv
            .next()
            .await
            .ok_or_else(|| anyhow::anyhow!("Connection dropped"))?;

        conn
    };

    #[cfg(not(target_family = "wasm"))]
    let (datagrams_recv, send_datagram, mut handshake_stream_send, handshake_stream_recv) = {
        let handshake_stream = conn.connection.open_bi().await?;

        log::debug!("opened stream");

        let handshake_stream_send =
            async_bincode::AsyncBincodeWriter::<_, ni_ty::protocol::HandshakeMessageC2S, _>::from(
                handshake_stream.0,
            )
            .for_async();
        let handshake_stream_recv = async_bincode::AsyncBincodeReader::<
            _,
            ni_ty::protocol::HandshakeMessageS2C,
        >::from(handshake_stream.1);

        let send_datagram = |data| conn.connection.send_datagram(data);

        (
            conn.datagrams,
            send_datagram,
            handshake_stream_send,
            handshake_stream_recv,
        )
    };

    #[cfg(target_family = "wasm")]
    let (
        datagrams_recv,
        send_datagram,
        handshake_stream_send,
        handshake_stream_recv,
        game_stream_send,
        game_stream_recv,
    ) = {
        use serde::Serialize;
        struct WSBiQuasiSink<'a, T: Serialize> {
            conn: &'a wasm_sockets::EventClient,
            id: i8,
            _p: std::marker::PhantomData<T>,
        }

        impl<'a, T: Serialize> WSBiQuasiSink<'a, T> {
            pub async fn send(&self, message: T) -> Result<(), anyhow::Error> {
                let mut data = bi_bincode_options().serialize(&message)?;

                data.push(self.id.to_ne_bytes()[0]);

                match self.conn.send_binary(data) {
                    Ok(_) => Ok(()),
                    Err(err) => Err(anyhow::anyhow!(
                        "Failed to send WebSocket message: {:?}",
                        err
                    )),
                }
            }
        }

        let (datagrams_recv_send, datagrams_recv_recv) =
            futures_channel::mpsc::channel::<Result<Vec<u8>, std::convert::Infallible>>(2);
        let (handshake_stream_recv_send, handshake_stream_recv_recv) =
            futures_channel::mpsc::unbounded::<Result<_, anyhow::Error>>();
        let (game_stream_recv_send, game_stream_recv_recv) =
            futures_channel::mpsc::unbounded::<Result<_, anyhow::Error>>();

        let datagrams_recv_send = std::cell::RefCell::new(datagrams_recv_send);
        let handshake_stream_recv_send = std::cell::RefCell::new(handshake_stream_recv_send);
        let game_stream_recv_send = std::cell::RefCell::new(game_stream_recv_send);

        conn.set_on_message(Some(Box::new(move |_, msg| {
            if let wasm_sockets::Message::Binary(mut data) = msg {
                if let Some(id) = data.pop() {
                    let id = i8::from_ne_bytes([id]);
                    match id {
                        0 => {
                            let _ = datagrams_recv_send.borrow_mut().try_send(Ok(data));
                        }
                        1 => {
                            let _ = handshake_stream_recv_send.borrow_mut().unbounded_send(
                                bi_bincode_options().deserialize(&data).map_err(Into::into),
                            );
                        }
                        -1 => {
                            let _ = game_stream_recv_send.borrow_mut().unbounded_send(
                                bi_bincode_options().deserialize(&data).map_err(Into::into),
                            );
                        }
                        _ => {}
                    }
                }
            }
        })));

        let send_datagram = |mut data: Vec<u8>| {
            data.push(0);
            match conn.send_binary(data) {
                Ok(_) => Ok(()),
                Err(err) => Err(anyhow::anyhow!(
                    "Failed to send WebSocket message: {:?}",
                    err
                )),
            }
        };

        let handshake_stream_send = WSBiQuasiSink {
            conn: &conn,
            id: 1,
            _p: std::marker::PhantomData,
        };

        let game_stream_send = WSBiQuasiSink {
            conn: &conn,
            id: -1,
            _p: std::marker::PhantomData,
        };

        (
            datagrams_recv_recv,
            send_datagram,
            handshake_stream_send,
            handshake_stream_recv_recv,
            game_stream_send,
            game_stream_recv_recv,
        )
    };

    log::debug!("connected");

    let hello_msg = ni_ty::protocol::HandshakeMessageC2S::Hello {
        name: (*settings_mutex.lock().unwrap()).name.clone(),
        game_id,
        new_game_public,
        protocol_version: ni_ty::protocol::PROTOCOL_VERSION,
        min_protocol_version: ni_ty::protocol::PROTOCOL_VERSION,
    };
    handshake_stream_send.send(hello_msg).await?;

    log::debug!("sent hello");

    let (first_message, handshake_stream_recv) = handshake_stream_recv.into_future().await;
    let first_message = first_message.ok_or(anyhow::anyhow!("Failed to complete handshake"))??;

    let _ = (handshake_stream_recv, handshake_stream_send);

    #[allow(irrefutable_let_patterns)]
    if let ni_ty::protocol::HandshakeMessageS2C::Hello = first_message {
    } else {
        anyhow::bail!("Unknown handshake response");
    }

    log::debug!("aaa");

    #[cfg(not(target_family = "wasm"))]
    let (mut game_stream_send, game_stream_recv) = {
        let (game_stream_res, _bi_streams) = conn.bi_streams.into_future().await;
        let game_stream = game_stream_res.ok_or(anyhow::anyhow!("Missing game stream"))??;

        log::debug!("bbb");

        let game_stream_send =
            async_bincode::AsyncBincodeWriter::<_, ni_ty::protocol::GameMessageC2S, _>::from(
                game_stream.0,
            )
            .for_async();
        let game_stream_recv = async_bincode::AsyncBincodeReader::<
            _,
            ni_ty::protocol::GameMessageS2C,
        >::from(game_stream.1);

        (game_stream_send, game_stream_recv)
    };

    log::debug!("wat");

    let (send_leave, recv_leave) = futures_channel::oneshot::channel();

    let x = if let futures_util::future::Either::Left((Err(err), _)) = futures_util::future::select(
        Box::pin(futures_util::future::try_join4(
            async move {
                while let Some(msg) = game_msg_recv.next().await {
                    match msg {
                        ConnectionMessage::Game(msg) => {
                            log::debug!("sending {:?}", msg);
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
                let interval = wasm_timer::Interval::new(std::time::Duration::from_millis(50));

                interval
                    .map::<anyhow::Result<()>, _>(Ok)
                    .try_fold(0u32, move |mut seq, _| async move {
                        let mut lock = info_mutex.lock().unwrap();

                        if let Some(shared) = lock.as_info_mut() {
                            if shared.game.hand.is_some() {
                                let hand_extra = shared.hand_extra.as_ref().unwrap();
                                if let Some(mouse_pos) = hand_extra.last_mouse_position {
                                    send_datagram(
                                        bincode::serialize(
                                            &ni_ty::protocol::DatagramMessageC2S::UpdateMouseState {
                                                seq,
                                                state: ni_ty::MouseState {
                                                    position: mouse_pos,
                                                    held: hand_extra.my_held_state,
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

                        Ok(seq)
                    })
                    .await?;

                // allows inferring return type
                #[allow(unreachable_code)]
                Ok(())
            },
            async {
                datagrams_recv
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
                                if let Some(shared) = (*lock).as_info_mut() {
                                    if let Some(hand_extra) = shared.hand_extra.as_mut() {
                                        let mouse_state =
                                            &mut hand_extra.mouse_states[player_idx as usize];
                                        if match mouse_state {
                                            Some(state) => state.0 < seq,
                                            None => true,
                                        } {
                                            *mouse_state = Some((seq, state));
                                        }
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

                        log::debug!("received {:?}", msg);

                        match msg {
                            GameMessageS2C::Joined {
                                info,
                                your_player_id,
                            } => {
                                *info_mutex.lock().unwrap() =
                                    ConnectionState::Connected(SharedInfo {
                                        hand_extra: info.hand.as_ref().map(|hand| {
                                            crate::HandExtra::new(hand.players().len())
                                        }),
                                        game: info,
                                        my_player_id: your_player_id,
                                        server_id,
                                        new_end_scores: None,
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
                            GameMessageS2C::HandInit { info, delay } => {
                                let mut lock = info_mutex.lock().unwrap();
                                let shared = (*lock).as_info_mut().unwrap();

                                let mut hand_extra = crate::HandExtra::new(info.players().len());
                                hand_extra.expected_start_time =
                                    Some(wasm_timer::Instant::now() + delay);
                                shared.hand_extra = Some(hand_extra);

                                shared.game.hand = Some(info);
                            }
                            GameMessageS2C::PlayerHandAction { player, action } => {
                                let mut lock = info_mutex.lock().unwrap();
                                let shared = lock.as_info_mut().unwrap();

                                let hand = shared.game.hand.as_mut().unwrap();
                                let hand_extra = shared.hand_extra.as_mut().unwrap();

                                if let Some(my_player_idx) = hand
                                    .players()
                                    .iter()
                                    .position(|player| player.player_id() == shared.my_player_id)
                                {
                                    if player == my_player_idx as u8 {
                                        // my move, check if matches expected

                                        while let Some(front) =
                                            hand_extra.pending_actions.pop_front()
                                        {
                                            if front == action {
                                                break;
                                            }
                                        }
                                    }
                                }

                                hand.apply(Some(player), action).unwrap();
                            }
                            GameMessageS2C::ServerHandAction { action } => {
                                let mut lock = info_mutex.lock().unwrap();
                                let shared = lock.as_info_mut().unwrap();

                                let hand = shared.game.hand.as_mut().unwrap();

                                hand.apply(None, action).unwrap();

                                if matches!(action, ni_ty::HandAction::ShuffleStock { .. }) {
                                    shared.hand_extra.as_mut().unwrap().stalled = false;
                                }
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

                                shared.hand_extra = None;
                            }
                            GameMessageS2C::HandStalled => {
                                let mut lock = info_mutex.lock().unwrap();
                                let shared = lock.as_info_mut().unwrap();

                                let hand_extra = shared.hand_extra.as_mut().unwrap();

                                hand_extra.stalled = true;
                            }
                            GameMessageS2C::HandStallCancel => {
                                let mut lock = info_mutex.lock().unwrap();
                                let shared = lock.as_info_mut().unwrap();

                                let hand_extra = shared.hand_extra.as_mut().unwrap();

                                hand_extra.stalled = false;
                            }
                            GameMessageS2C::GameEnd => {
                                let mut lock = info_mutex.lock().unwrap();
                                let shared = lock.as_info_mut().unwrap();

                                let mut scores: Vec<_> = shared
                                    .game
                                    .players
                                    .iter_mut()
                                    .map(|(key, player)| {
                                        let score = player.score;
                                        player.score = 0;

                                        (*key, score)
                                    })
                                    .collect();

                                scores.sort_by_key(|x| -x.1);

                                shared.new_end_scores = Some(scores);
                            }
                            GameMessageS2C::NewMasterPlayer { player } => {
                                let mut lock = info_mutex.lock().unwrap();
                                let shared = lock.as_info_mut().unwrap();

                                shared.game.master_player = player;
                            }
                            GameMessageS2C::HandStart => {
                                let mut lock = info_mutex.lock().unwrap();
                                let shared = lock.as_info_mut().unwrap();
                                let hand = shared.game.hand.as_mut().unwrap();

                                hand.started = true;
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
    };

    x
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
