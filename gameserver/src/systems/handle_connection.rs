use crate::{
    GlobalState, PlayerController, ServerGamePlayerState, ServerGameState, ServerHandState,
};
use futures_util::{SinkExt, Stream, StreamExt, TryStreamExt};
use nertsio_types as ni_ty;
use rand::Rng;
use std::future::Future;
use std::sync::Arc;

const HAND_START_DELAY: std::time::Duration = std::time::Duration::from_secs(5);

fn start_hand(server_game_state: &mut ServerGameState, global_state: &Arc<GlobalState>) {
    let new_hand = ni_ty::HandState::generate(server_game_state.players.keys().copied());
    server_game_state.hand = Some(ServerHandState {
        hand: new_hand.clone(),
        mouse_states: vec![None; new_hand.players().len()],
        stalled_count: 0,
        sent_stall: false,
    });

    let delay = HAND_START_DELAY;

    server_game_state.send_to_all(ni_ty::protocol::GameMessageS2C::HandInit {
        info: new_hand,
        delay,
    });

    let game_id = server_game_state.game_id;
    let global_state = global_state.clone();
    tokio::spawn(async move {
        tokio::time::sleep(delay).await;

        if let Some(mut server_game_state) = global_state.games.get_mut(&game_id) {
            if let Some(hand_state) = server_game_state.hand.as_mut() {
                if !hand_state.hand.started {
                    hand_state.hand.started = true;

                    server_game_state.send_to_all(ni_ty::protocol::GameMessageS2C::HandStart);
                }
            }
        }
    });
}

fn maybe_start_hand(server_game_state: &mut ServerGameState, global_state: &Arc<GlobalState>) {
    if server_game_state.hand.is_none()
        && !server_game_state.players.is_empty()
        && server_game_state
            .players
            .values()
            .all(|player| player.ready)
    {
        // all ready, start hand

        start_hand(server_game_state, global_state);
    }
}

async fn handle_connection<
    C: crate::connection::Connection,
    E: Into<anyhow::Error> + Sync + Send + 'static,
>(
    global_state: Arc<GlobalState>,
    connecting: impl Future<Output = Result<C, E>>,
) -> Result<(), anyhow::Error>
where
    C::Handle: Clone,
{
    let mut connection = connecting.await.map_err(Into::into)?;

    let handle = connection.create_handle();

    let maintenance_stream_res = connection.accept_bi_stream().await;
    let maintenance_stream =
        maintenance_stream_res.ok_or(anyhow::anyhow!("Stream closed without handshake"))??;

    let mut maintenance_stream_send = Box::pin(
        maintenance_stream
            .0
            .sink_map_err(anyhow::Error::from)
            .with(|msg| async move {
                use bincode::Options;
                use bytes::BufMut;

                let mut dest = bytes::BytesMut::new().writer();

                bincode::options()
                    .with_limit(u32::max_value() as u64)
                    .allow_trailing_bytes()
                    .serialize_into(&mut dest, &msg)?;

                Result::<_, anyhow::Error>::Ok(dest.into_inner().freeze())
            }),
    );
    let maintenance_stream_recv = Box::pin(maintenance_stream.1.map_err(Into::into).and_then(
        |data| async move {
            use bincode::Options;

            bincode::options()
                .with_limit(u32::max_value() as u64)
                .allow_trailing_bytes()
                .deserialize(&data)
                .map_err(anyhow::Error::from)
        },
    ));

    println!("init");

    let (first_message, mut maintenance_stream_recv) = maintenance_stream_recv.into_future().await;

    println!("hmm {:?}", first_message);

    let first_message = first_message.ok_or(anyhow::anyhow!("Stream closed without Hello"))??;

    println!("first: {:?}", first_message);

    let (name, game_id, new_game_public) = if let ni_ty::protocol::MaintenanceMessageC2S::Hello {
        name,
        game_id,
        new_game_public,
        protocol_version,
        min_protocol_version,
    } = first_message
    {
        use crate::connection::ConnectionHandle;

        if ni_ty::protocol::PROTOCOL_VERSION < min_protocol_version {
            handle.close(ni_ty::protocol::CLOSE_TOO_NEW);
            anyhow::bail!("Mismatched protocol");
        }
        if protocol_version < crate::MIN_PROTOCOL_VERSION {
            handle.close(ni_ty::protocol::CLOSE_TOO_OLD);
            anyhow::bail!("Mismatched protocol");
        }
        (name, game_id, new_game_public == Some(true))
    } else {
        anyhow::bail!("Wrong first handshake message");
    };

    let (game_stream_send_channel_send, mut game_stream_send_channel_recv) =
        tokio::sync::mpsc::unbounded_channel();

    let (player_id, game_state, game_id) = {
        let (mut server_game_state, game_id) = {
            if let Some(game_id) = game_id {
                (
                    global_state
                        .games
                        .get_mut(&game_id)
                        .ok_or(anyhow::anyhow!("Unknown game"))?,
                    game_id,
                )
            } else {
                loop {
                    let game_id = u32::from(rand::thread_rng().gen::<u16>());
                    if let dashmap::mapref::entry::Entry::Vacant(entry) =
                        global_state.games.entry(game_id)
                    {
                        break (
                            entry.insert(ServerGameState::new(game_id, new_game_public)),
                            game_id,
                        );
                    }
                }
            }
        };

        let player_id = loop {
            let player_id = rand::thread_rng().gen();
            if let std::collections::hash_map::Entry::Vacant(entry) =
                server_game_state.players.entry(player_id)
            {
                entry.insert(ServerGamePlayerState {
                    name,
                    ready: false,
                    score: 0,
                    controller: PlayerController::Network {
                        game_stream_send_channel: game_stream_send_channel_send,
                        connection: Box::new(handle.clone()),
                    },
                });

                break player_id;
            }
        };

        let master_player = match server_game_state.master_player {
            Some(master) => master,
            None => {
                assert_eq!(server_game_state.players.len(), 1);

                server_game_state.master_player = Some(player_id);
                player_id
            }
        };

        let game_state = ni_ty::GameState {
            id: game_id,
            players: server_game_state
                .players
                .iter()
                .map(|(key, value)| (*key, value.to_common_state()))
                .collect(),
            hand: server_game_state
                .hand
                .as_ref()
                .map(|hand| hand.hand.clone()),
            master_player,
        };

        (player_id, game_state, game_id)
    };

    let send_to_others = move |server_game_state: &ServerGameState,
                               msg: ni_ty::protocol::GameMessageS2C| {
        for (id, server_player_state) in &server_game_state.players {
            if *id != player_id {
                if let PlayerController::Network {
                    ref game_stream_send_channel,
                    ..
                } = server_player_state.controller
                {
                    println!("sending {:?} to {}", msg, id);
                    if let Err(err) = game_stream_send_channel.send(msg.clone()) {
                        eprintln!("Failed to queue update to player: {:?}", err);
                    }
                }
            }
        }
    };

    let res = async {
        {
            let server_game_state = global_state
                .games
                .get(&game_id)
                .ok_or(anyhow::anyhow!("Unknown game"))?;

            send_to_others(
                &server_game_state,
                ni_ty::protocol::GameMessageS2C::PlayerJoin {
                    id: player_id,
                    info: server_game_state
                        .players
                        .get(&player_id)
                        .unwrap()
                        .to_common_state(),
                },
            );
        }

        maintenance_stream_send
            .send(ni_ty::protocol::MaintenanceMessageS2C::Hello)
            .await?;

        let game_stream = connection.start_bi_stream().await?;

        let mut game_stream_send = Box::pin(game_stream.0.sink_map_err(anyhow::Error::from).with(|msg| async move {
            use bincode::Options;
            use bytes::BufMut;

            let mut dest = bytes::BytesMut::new().writer();

            bincode::options()
                .with_limit(u32::max_value() as u64)
                .allow_trailing_bytes()
                .serialize_into(&mut dest, &msg)?;

            Result::<_, anyhow::Error>::Ok(dest.into_inner().freeze())
        }));
        let game_stream_recv = game_stream.1.map_err(Into::into).and_then(|data| async move {
            use bincode::Options;

            Ok(bincode::options()
                .with_limit(u32::max_value() as u64)
                .allow_trailing_bytes()
                .deserialize(&data)?)
        });

        game_stream_send
            .send(ni_ty::protocol::GameMessageS2C::Joined {
                info: game_state,
                your_player_id: player_id,
            })
            .await?;

        println!("iedkeinstrkdie");

        let (leave_send, mut leave_recv) = tokio::sync::oneshot::channel();

        futures_util::try_join!(
            async {
                println!("denrstdensrtkenaa");
                loop {
                    use futures_util::future::Either;

                    leave_recv = {
                        let res = futures_util::future::select(
                            Box::pin(game_stream_send_channel_recv.recv()),
                            leave_recv,
                        )
                        .await;

                        match res {
                            Either::Left((Some(msg), leave_recv)) => {
                                println!("passing {:?} to {}", msg, player_id);
                                game_stream_send.send(msg).await?;
                                leave_recv
                            }
                            Either::Left((None, _))
                            | Either::Right((Ok(()), _))
                            | Either::Right((Err(_), _)) => break,
                        }
                    };
                }
                println!("and no more");
                Result::<_, anyhow::Error>::Ok(())
            },
            async {
                let global_state = global_state.clone();
                loop {
                    use ni_ty::protocol::DatagramMessageC2S;

                    let bytes = connection.read_datagram().await?;

                    let msg: DatagramMessageC2S = bincode::deserialize(&bytes)?;
                    match msg {
                        DatagramMessageC2S::UpdateMouseState { seq, state } => {
                            let mut server_game_state = global_state
                                .games
                                .get_mut(&game_id)
                                .ok_or(anyhow::anyhow!("Unknown game"))?;

                            if let Some(ref mut hand_state) = server_game_state.hand {
                                if let Some(player_idx) = hand_state.hand.players().iter().position(|player| player.player_id() == player_id) {
                                    if match hand_state.mouse_states[player_idx] {
                                        Some(ref state) => state.0 < seq,
                                            None => true,
                                    } {
                                        hand_state.mouse_states[player_idx] = Some((seq, state.clone()));

                                        let out_msg: bytes::Bytes = bincode::serialize(&ni_ty::protocol::DatagramMessageS2C::UpdateMouseState {
                                            player_idx: player_idx as u8,
                                            seq,
                                            state,
                                        }).unwrap().into();

                                        for (id, server_player_state) in &server_game_state.players {
                                            if *id != player_id {
                                                if let PlayerController::Network { ref connection, .. } = server_player_state.controller {
                                                    if let Err(err) = connection.send_datagram(out_msg.clone())
                                                    {
                                                        eprintln!("Failed to queue update to player: {:?}", err);
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                // required for type inference
                #[allow(unreachable_code)]
                Ok(())
            },
            async {
                let global_state = global_state.clone();
                game_stream_recv
                    .try_for_each(move |msg| {
                        println!("received {:?}", msg);
                        let global_state = global_state.clone();
                        async move {
                            use ni_ty::protocol::GameMessageC2S;

                            match msg {
                                GameMessageC2S::UpdateSelfReady { value } => {
                                    let mut server_game_state = global_state
                                        .games
                                        .get_mut(&game_id)
                                        .ok_or(anyhow::anyhow!("Unknown game"))?;

                                    if server_game_state.hand.is_none() {
                                        let server_player_state =
                                            server_game_state.players.get_mut(&player_id).unwrap();
                                        if server_player_state.ready != value {
                                            server_player_state.ready = value;

                                            send_to_others(
                                                &server_game_state,
                                                ni_ty::protocol::GameMessageS2C::PlayerUpdateReady {
                                                    id: player_id,
                                                    value,
                                                },
                                            );
                                        }

                                        maybe_start_hand(&mut server_game_state, &global_state);
                                    }
                                }
                                GameMessageC2S::ForceStart => {
                                    let mut server_game_state = global_state
                                        .games
                                        .get_mut(&game_id)
                                        .ok_or(anyhow::anyhow!("Unknown game"))?;

                                    if server_game_state.master_player == Some(player_id) {
                                        if server_game_state.hand.is_none() && !server_game_state.players.is_empty() {
                                            start_hand(&mut server_game_state, &global_state);
                                        }
                                    }
                                }
                                GameMessageC2S::ApplyHandAction { action } => {
                                    let mut server_game_state = global_state
                                        .games
                                        .get_mut(&game_id)
                                        .ok_or(anyhow::anyhow!("Unknown game"))?;

                                    if let Some(ref mut hand_state) = server_game_state.hand {
                                        if hand_state.hand.started {
                                            if let Some(player_idx) = hand_state.hand.players().iter().position(|player| player.player_id() == player_id) {
                                                match hand_state.hand.apply(Some(player_idx as u8), action) {
                                                    Err(_) => {
                                                        println!("cannot apply action {:?}", action);
                                                    }
                                                    Ok(_) => {
                                                        server_game_state.send_to_all(ni_ty::protocol::GameMessageS2C::PlayerHandAction { player: player_idx as u8, action });

                                                        if action.should_reset_stall() {
                                                            let hand_state = server_game_state.hand.as_mut().unwrap();
                                                            hand_state.stalled_count = 0;
                                                            if hand_state.sent_stall {
                                                                hand_state.sent_stall = false;
                                                                server_game_state.send_to_all(ni_ty::protocol::GameMessageS2C::HandStallCancel);
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                                GameMessageC2S::CallNerts => {
                                    let mut server_game_state = global_state
                                        .games
                                        .get_mut(&game_id)
                                        .ok_or(anyhow::anyhow!("Unknown game"))?;

                                    server_game_state.handle_nerts_call(player_id, &global_state);
                                }
                                GameMessageC2S::AddBot => {
                                    let mut server_game_state = global_state
                                        .games
                                        .get_mut(&game_id)
                                        .ok_or(anyhow::anyhow!("Unknown game"))?;

                                    if server_game_state.master_player == Some(player_id) {
                                        let bot_id = loop {
                                            let bot_id = rand::thread_rng().gen();
                                            if let std::collections::hash_map::Entry::Vacant(entry) = server_game_state.players.entry(bot_id) {
                                                let bot_name = format!("Bot {}", bot_id);

                                                entry.insert(
                                                    ServerGamePlayerState {
                                                        name: bot_name,
                                                        ready: true,
                                                        score: 0,
                                                        controller: PlayerController::Bot {
                                                            mouse_state: ni_ty::MouseState {
                                                                held: None,
                                                                position: Default::default(),
                                                            },
                                                            plan: None,
                                                            target: Default::default(),
                                                            seq: 0,
                                                        },
                                                    },
                                                );

                                                break bot_id;
                                            }
                                        };

                                        server_game_state.send_to_all(
                                            ni_ty::protocol::GameMessageS2C::PlayerJoin {
                                                id: bot_id,
                                                info: server_game_state
                                                    .players
                                                    .get(&bot_id)
                                                    .unwrap()
                                                    .to_common_state(),
                                            },
                                        );
                                    }
                                }
                                GameMessageC2S::KickPlayer { player } => {
                                    let mut server_game_state = global_state
                                        .games
                                        .get_mut(&game_id)
                                        .ok_or(anyhow::anyhow!("Unknown game"))?;

                                    if server_game_state.master_player == Some(player_id) {
                                        if let Some(target) = server_game_state.players.get(&player) {
                                            match &target.controller {
                                                PlayerController::Network { connection, .. } => {
                                                    connection.close(ni_ty::protocol::CLOSE_KICK);
                                                }
                                                PlayerController::Bot { .. } => {
                                                    server_game_state.players.remove(&player);
                                                    server_game_state.send_to_all(
                                                        ni_ty::protocol::GameMessageS2C::PlayerLeave { id: player },
                                                    );
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            Result::<_, anyhow::Error>::Ok(())
                        }
                    })
                    .await?;

                if let Err(err) = leave_send.send(()) {
                    eprintln!("Failed to send leave event: {:?}", err);
                }

                Ok(())
            },
            async move {
                while let Some(msg) = maintenance_stream_recv.try_next().await? {
                    use ni_ty::protocol::MaintenanceMessageC2S;

                    match msg {
                        MaintenanceMessageC2S::Ping => {
                            maintenance_stream_send.send(ni_ty::protocol::MaintenanceMessageS2C::Pong).await?;
                        }
                        _ => {
                            anyhow::bail!("Unexpected maintenance message");
                        }
                    }
                }

                Ok(())
            }
        )?;

        Ok(())
    }
    .await;

    let mut server_game_state = global_state
        .games
        .get_mut(&game_id)
        .ok_or(anyhow::anyhow!("Unknown game"))?;

    server_game_state.players.remove(&player_id);
    send_to_others(
        &server_game_state,
        ni_ty::protocol::GameMessageS2C::PlayerLeave { id: player_id },
    );

    if Some(player_id) == server_game_state.master_player {
        // master left, need to assign a new one

        let new_master = server_game_state.players.keys().next().copied();
        server_game_state.master_player = new_master;
        if let Some(new_master) = new_master {
            send_to_others(
                &server_game_state,
                ni_ty::protocol::GameMessageS2C::NewMasterPlayer { player: new_master },
            );
        }
    }

    maybe_start_hand(&mut server_game_state, &global_state);

    res
}

pub(crate) async fn run<
    C: crate::connection::Connection + Send + Sync,
    E: Into<anyhow::Error> + Sync + Send + 'static,
    F: Future<Output = Result<C, E>> + Send + 'static,
>(
    global_state: Arc<GlobalState>,
    incoming: impl Stream<Item = F>,
) where
    C::Handle: Clone,
{
    incoming
        .for_each(move |connecting| {
            let global_state = global_state.clone();
            tokio::spawn(async {
                let res = handle_connection(global_state, connecting).await;
                if let Err(err) = res {
                    eprintln!("Failed to handle connection: {:?}", err);
                }
            });

            futures_util::future::ready(())
        })
        .await;
}
