use futures_util::SinkExt;
use futures_util::{StreamExt, TryStreamExt};
use macroquad::logging as log;
use nertsio_types as ni_ty;
use std::sync::{Arc, Mutex};
use xwt_core::traits::*;

use crate::{ConnectionState, SharedInfo};

#[cfg(target_family = "wasm")]
use async_bincode::futures as async_bincode_current;
#[cfg(not(target_family = "wasm"))]
use async_bincode::tokio as async_bincode_current;

const PING_LOOP_DELAY_MINIMUM: std::time::Duration = std::time::Duration::from_secs(1);
const PING_LOOP_DELAY_STANDARD: std::time::Duration = std::time::Duration::from_secs(10);

#[allow(clippy::enum_variant_names)]
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

/// Used to trigger sounds
pub enum ConnectionEvent {
    HandInit,
    PlayerHandAction(ni_ty::HandAction),
    NertsCalled,
}

pub(crate) async fn handle_connection(
    http_client: &reqwest::Client,
    coordinator_url: &str,
    connection_type: ConnectionType<'_>,
    info_mutex: &std::sync::Mutex<ConnectionState>,
    mut game_msg_recv: futures_channel::mpsc::UnboundedReceiver<ConnectionMessage>,
    settings_mutex: Arc<Mutex<crate::Settings>>,
    events_send: futures_channel::mpsc::UnboundedSender<ConnectionEvent>,
    async_rt: crate::AsyncRt,
) -> Result<(), anyhow::Error> {
    let (server, game_id, new_game_public) = match connection_type {
        ConnectionType::CreateGame { public } => {
            let resp = http_client
                .post(format!(
                    "{}servers:pick_for_new_game?protocol_version={}&min_protocol_version={}",
                    coordinator_url,
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
                .get(format!("{}servers/{}", coordinator_url, server_id))
                .send()
                .await?
                .error_for_status()?;

            let resp: ni_ty::protocol::ServerConnectionInfo = resp.json().await?;

            (resp, Some(game_id), None)
        }
    };

    let server_id = server.server_id;

    let conn = {
        #[cfg(target_family = "wasm")]
        let endpoint = xwt::current::Endpoint::default();

        #[cfg(not(target_family = "wasm"))]
        let endpoint = xwt::current::Endpoint(wtransport::Endpoint::client(
            wtransport::ClientConfig::builder()
                .with_bind_default()
                .with_no_cert_validation()
                .build(),
        )?);

        let connecting = endpoint
            .connect(&format!(
                "https://{}:{}",
                server
                    .hostname
                    .unwrap_or_else(|| server.address_ipv4.ip().to_string().into()),
                server
                    .web_port
                    .ok_or(anyhow::anyhow!("No web_port for server"))?
            ))
            .await
            .map_err(error_send)?;
        Arc::new(connecting.wait_connect().await?)
    };

    let (datagrams_recv, send_datagram, mut maintenance_stream_send, maintenance_stream_recv) =
        {
            use xwt_core::datagram::{Receive, Send};

            let maintenance_stream = conn.open_bi().await.map_err(error_send)?.wait_bi().await?;

            log::debug!("opened stream");

            let maintenance_stream_send =
                async_bincode_current::AsyncBincodeWriter::<
                    _,
                    ni_ty::protocol::MaintenanceMessageC2S,
                    _,
                >::from(hack_send_stream(Box::new(maintenance_stream.0)))
                .for_async();
            let maintenance_stream_recv =
                async_bincode_current::AsyncBincodeReader::<
                    _,
                    ni_ty::protocol::MaintenanceMessageS2C,
                >::from(hack_recv_stream(Box::new(maintenance_stream.1)));

            let (datagrams_out_tx, datagrams_out_rx) = futures_channel::mpsc::unbounded();

            {
                let conn = conn.clone();
                async_rt.spawn(async move {
                    datagrams_out_rx
                        .for_each(|bytes| async {
                            if let Err(err) = conn.send_datagram(bytes).await {
                                eprintln!("Failed to send datagram: {:?}", err);
                            }
                        })
                        .await;
                });
            }

            let send_datagram = move |data| datagrams_out_tx.unbounded_send(data);

            let datagrams_recv = futures_util::stream::unfold((), |()| async {
                Some((conn.receive_datagram().await, ()))
            });

            (
                datagrams_recv,
                send_datagram,
                maintenance_stream_send,
                maintenance_stream_recv,
            )
        };

    log::debug!("connected");

    let hello_msg = ni_ty::protocol::MaintenanceMessageC2S::Hello {
        name: settings_mutex.lock().unwrap().name.clone(),
        game_id,
        new_game_public,
        protocol_version: ni_ty::protocol::PROTOCOL_VERSION,
        min_protocol_version: ni_ty::protocol::PROTOCOL_VERSION,
    };
    maintenance_stream_send.send(hello_msg).await?;

    log::debug!("sent hello");

    let (first_message, mut maintenance_stream_recv) = maintenance_stream_recv.into_future().await;
    let first_message = first_message.ok_or(anyhow::anyhow!("Failed to complete handshake"))??;

    if let ni_ty::protocol::MaintenanceMessageS2C::Hello = first_message {
    } else {
        anyhow::bail!("Unknown handshake response");
    }

    log::debug!("aaa");

    let (mut game_stream_send, game_stream_recv) = {
        let game_stream = conn.accept_bi().await.map_err(error_send)?;

        log::debug!("bbb");

        let game_stream_send = async_bincode_current::AsyncBincodeWriter::<
            _,
            ni_ty::protocol::GameMessageC2S,
            _,
        >::from(hack_send_stream(Box::new(game_stream.0)))
        .for_async();
        let game_stream_recv = async_bincode_current::AsyncBincodeReader::<
            _,
            ni_ty::protocol::GameMessageS2C,
        >::from(hack_recv_stream(Box::new(game_stream.1)));

        (game_stream_send, game_stream_recv)
    };

    log::debug!("wat");

    let (send_leave, recv_leave) = futures_channel::oneshot::channel();

    // required to make this compile
    #[allow(clippy::let_and_return)]
    let x = if let futures_util::future::Either::Left((Err(err), _)) = futures_util::future::select(
        Box::pin(futures_util::future::try_join5(
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
                let mut interval =
                    futures_ticker::Ticker::new(std::time::Duration::from_millis(50));
                let mut seq = 0;

                loop {
                    interval.next().await;

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
                                                held: hand_extra
                                                    .my_held_state
                                                    .as_ref()
                                                    .map(|x| x.info),
                                            },
                                        },
                                    )
                                    .unwrap(),
                                )?;

                                seq += 1;
                            }
                        }
                    }
                }

                // allows inferring return type
                #[allow(unreachable_code)]
                Ok(())
            },
            async {
                datagrams_recv
                    .map_err(error_send)
                    .try_for_each(|bytes| async move {
                        use ni_ty::protocol::DatagramMessageS2C;

                        let msg: DatagramMessageS2C = bincode::deserialize(bytes.as_ref())?;
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
                                        match mouse_state {
                                            Some(current_state) => {
                                                current_state.receive(seq, state)
                                            }
                                            None => {
                                                *mouse_state =
                                                    Some(crate::MouseState::new(seq, state));
                                            }
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
                    .try_for_each(|msg| {
                        let events_send = &events_send;
                        async move {
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
                                            ping: None,
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

                                    let mut hand_extra =
                                        crate::HandExtra::new(info.players().len());
                                    hand_extra.expected_start_time =
                                        Some(web_time::Instant::now() + delay);
                                    shared.hand_extra = Some(hand_extra);

                                    shared.game.hand = Some(info);

                                    if let Err(err) =
                                        events_send.unbounded_send(ConnectionEvent::HandInit)
                                    {
                                        eprintln!("unable to trigger HandInit event: {:?}", err);
                                    }
                                }
                                GameMessageS2C::PlayerHandAction { player, action } => {
                                    let mut lock = info_mutex.lock().unwrap();
                                    let shared = lock.as_info_mut().unwrap();

                                    let hand = shared.game.hand.as_mut().unwrap();
                                    let hand_extra = shared.hand_extra.as_mut().unwrap();

                                    if let Some(my_player_idx) =
                                        hand.players().iter().position(|player| {
                                            player.player_id() == shared.my_player_id
                                        })
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

                                    let _ = events_send
                                        .unbounded_send(ConnectionEvent::PlayerHandAction(action));
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

                                    let _ =
                                        events_send.unbounded_send(ConnectionEvent::NertsCalled);
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
                                GameMessageS2C::SettingsChanged { settings } => {
                                    let mut lock = info_mutex.lock().unwrap();
                                    let shared = lock.as_info_mut().unwrap();

                                    shared.game.settings = settings;
                                }
                            }

                            Ok(())
                        }
                    })
                    .await
            },
            async move {
                loop {
                    let start_time = web_time::Instant::now();
                    maintenance_stream_send
                        .send(ni_ty::protocol::MaintenanceMessageC2S::Ping)
                        .await?;

                    let msg = maintenance_stream_recv
                        .try_next()
                        .await?
                        .ok_or(anyhow::anyhow!("maintenance stream ended"))?;

                    if let ni_ty::protocol::MaintenanceMessageS2C::Pong = msg {
                        let end_time = web_time::Instant::now();

                        let ping = end_time - start_time;

                        info_mutex.lock().unwrap().as_info_mut().unwrap().ping = Some(ping);

                        let delay = if ping >= (PING_LOOP_DELAY_STANDARD - PING_LOOP_DELAY_MINIMUM)
                        {
                            PING_LOOP_DELAY_MINIMUM
                        } else {
                            PING_LOOP_DELAY_STANDARD - ping
                        };

                        futures_timer::Delay::new(delay).await;
                    } else {
                        anyhow::bail!("unexpected maintenance message");
                    }
                }

                // allows inferring return type
                #[allow(unreachable_code)]
                Ok(())
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

#[cfg(not(target_family = "wasm"))]
pub fn hack_send_stream(src: Box<dyn std::any::Any>) -> wtransport::SendStream {
    (*src.downcast::<xwt_wtransport::SendStream>().unwrap()).0
}

#[cfg(not(target_family = "wasm"))]
pub fn hack_recv_stream(src: Box<dyn std::any::Any>) -> wtransport::RecvStream {
    (*src.downcast::<xwt_wtransport::RecvStream>().unwrap()).0
}

#[cfg(target_family = "wasm")]
pub fn hack_send_stream(
    src: Box<dyn std::any::Any>,
) -> tokio_util::compat::Compat<xwt_web_sys::SendStream> {
    tokio_util::compat::TokioAsyncWriteCompatExt::compat_write(
        *src.downcast::<xwt_web_sys::SendStream>().unwrap(),
    )
}

#[cfg(target_family = "wasm")]
pub fn hack_recv_stream(
    src: Box<dyn std::any::Any>,
) -> tokio_util::compat::Compat<xwt_web_sys::RecvStream> {
    tokio_util::compat::TokioAsyncReadCompatExt::compat(
        *src.downcast::<xwt_web_sys::RecvStream>().unwrap(),
    )
}

#[cfg(not(target_family = "wasm"))]
fn error_send<T: Into<anyhow::Error>>(src: T) -> anyhow::Error {
    src.into()
}

#[cfg(target_family = "wasm")]
fn error_send(src: xwt_web_sys::Error) -> anyhow::Error {
    anyhow::anyhow!("Error in connection: {:?}", src)
}
