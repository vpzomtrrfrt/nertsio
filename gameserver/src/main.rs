use futures_util::sink::SinkExt;
use futures_util::stream::{StreamExt, TryStreamExt};
use nertsio_types as ni_ty;
use rand::Rng;
use std::collections::HashMap;
use std::io::Read;
use std::sync::Arc;

const MAX_PLAYERS: usize = 6;
const STALL_SEND_COUNT: u8 = 6;
const WIN_SCORE: i32 = 100;
const BOT_CURSOR_SPEED: f32 = 10.0;

#[derive(Clone)]
enum BotPlan {
    Action(ni_ty::HandAction),
    CallNerts,
}

impl From<ni_ty::HandAction> for BotPlan {
    fn from(src: ni_ty::HandAction) -> Self {
        BotPlan::Action(src)
    }
}

enum PlayerController {
    Network {
        game_stream_send_channel:
            tokio::sync::mpsc::UnboundedSender<ni_ty::protocol::GameMessageS2C>,
        connection: quinn::Connection,
    },
    Bot {
        mouse_state: ni_ty::MouseState,
        plan: Option<BotPlan>,
        target: (f32, f32),
        seq: u32,
    },
}

struct ServerGamePlayerState {
    name: String,
    ready: bool,
    score: i32,
    controller: PlayerController,
}

struct ServerHandState {
    hand: ni_ty::HandState,
    mouse_states: Vec<Option<(u32, ni_ty::MouseState)>>,
    stalled_count: u8,
    sent_stall: bool,
}

impl ServerGamePlayerState {
    pub fn to_common_state(&self) -> ni_ty::GamePlayerState {
        ni_ty::GamePlayerState {
            name: self.name.clone(),
            ready: self.ready,
            score: self.score,
        }
    }
}

struct ServerGameState {
    game_id: u32,
    players: HashMap<u8, ServerGamePlayerState>,
    hand: Option<ServerHandState>,
    public: bool,
    master_player: Option<u8>,
}

impl ServerGameState {
    pub fn new(game_id: u32, public: bool) -> Self {
        Self {
            game_id,
            players: Default::default(),
            hand: None,
            public,
            master_player: None,
        }
    }
}

struct GlobalState {
    games: dashmap::DashMap<u32, ServerGameState>,
}

fn send_to_all(server_game_state: &ServerGameState, msg: ni_ty::protocol::GameMessageS2C) {
    for (id, server_player_state) in &server_game_state.players {
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

fn handle_nerts_call(
    server_game_state: &mut ServerGameState,
    player_id: u8,
    global_state: &Arc<GlobalState>,
) {
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

    let game_id = server_game_state.game_id;

    if let Some(ref mut hand_state) = server_game_state.hand {
        if let Some(player_idx) = hand_state
            .hand
            .players()
            .iter()
            .position(|player| player.player_id() == player_id)
        {
            if hand_state.hand.players()[player_idx].nerts_stack().len() == 0
                && !hand_state.hand.nerts_called
            {
                hand_state.hand.nerts_called = true;
                send_to_others(
                    &server_game_state,
                    ni_ty::protocol::GameMessageS2C::NertsCalled {
                        player: player_idx as u8,
                    },
                );
                let _ = server_game_state;

                let global_state = global_state.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

                    if let Some(mut server_game_state) = global_state.games.get_mut(&game_id) {
                        if let Some(hand_state) = server_game_state.hand.take() {
                            let mut scores: Vec<_> = hand_state
                                .hand
                                .players()
                                .iter()
                                .map(|player| (player.nerts_stack().len() as i32) * (-2))
                                .collect();
                            for stack in hand_state.hand.lake_stacks() {
                                for card in stack.cards() {
                                    scores[card.owner_id as usize] += 1;
                                }
                            }

                            let mut now_won = false;

                            hand_state
                                .hand
                                .players()
                                .iter()
                                .zip(scores.iter())
                                .for_each(|(player, score)| {
                                    if let Some(info) =
                                        server_game_state.players.get_mut(&player.player_id())
                                    {
                                        info.score += score;

                                        if info.score >= WIN_SCORE {
                                            now_won = true;
                                        }
                                    }
                                });

                            for player in server_game_state.players.values_mut() {
                                player.ready = false;
                            }

                            send_to_all(
                                &server_game_state,
                                ni_ty::protocol::GameMessageS2C::HandEnd { scores },
                            );

                            if now_won {
                                for (_, player) in &mut server_game_state.players {
                                    player.score = 0;
                                }

                                send_to_all(
                                    &server_game_state,
                                    ni_ty::protocol::GameMessageS2C::GameEnd,
                                );
                            }

                            let mut bots_ready = Vec::new();
                            for (key, player) in server_game_state.players.iter_mut() {
                                if let PlayerController::Bot { .. } = player.controller {
                                    player.ready = true;
                                    bots_ready.push(*key);
                                }
                            }

                            for id in bots_ready {
                                send_to_all(
                                    &server_game_state,
                                    ni_ty::protocol::GameMessageS2C::PlayerUpdateReady {
                                        id,
                                        value: true,
                                    },
                                );
                            }
                        }
                    }
                });
            }
        }
    }
}

fn maybe_start_hand(server_game_state: &mut ServerGameState) {
    if server_game_state.hand.is_none()
        && server_game_state
            .players
            .values()
            .all(|player| player.ready)
    {
        // all ready, start hand

        let new_hand = ni_ty::HandState::generate(server_game_state.players.keys().copied());
        server_game_state.hand = Some(ServerHandState {
            hand: new_hand.clone(),
            mouse_states: vec![None; new_hand.players().len()],
            stalled_count: 0,
            sent_stall: false,
        });
        send_to_all(
            &server_game_state,
            ni_ty::protocol::GameMessageS2C::HandStart { info: new_hand },
        );
    }
}

async fn handle_connection(
    global_state: Arc<GlobalState>,
    connecting: quinn::Connecting,
) -> Result<(), anyhow::Error> {
    let connection = connecting.await?;

    let (handshake_stream_res, _bi_streams) = connection.bi_streams.into_future().await;
    let handshake_stream =
        handshake_stream_res.ok_or(anyhow::anyhow!("Stream closed without handshake"))??;

    let mut handshake_stream_send =
        async_bincode::AsyncBincodeWriter::<_, ni_ty::protocol::HandshakeMessageS2C, _>::from(
            handshake_stream.0,
        )
        .for_async();
    let handshake_stream_recv = async_bincode::AsyncBincodeReader::<
        _,
        ni_ty::protocol::HandshakeMessageC2S,
    >::from(handshake_stream.1);

    println!("init");

    let (first_message, handshake_stream_recv) = handshake_stream_recv.into_future().await;

    println!("hmm {:?}", first_message);

    let first_message = first_message.ok_or(anyhow::anyhow!("Stream closed without Hello"))??;

    println!("first: {:?}", first_message);

    let _ = handshake_stream_recv;

    #[allow(irrefutable_let_patterns)]
    let (name, game_id, new_game_public) = if let ni_ty::protocol::HandshakeMessageC2S::Hello {
        name,
        game_id,
        new_game_public,
        protocol_version,
        min_protocol_version,
    } = first_message
    {
        if ni_ty::protocol::PROTOCOL_VERSION < min_protocol_version
            || protocol_version < ni_ty::protocol::PROTOCOL_VERSION
        {
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
            if !server_game_state.players.contains_key(&player_id) {
                server_game_state.players.insert(
                    player_id,
                    ServerGamePlayerState {
                        name,
                        ready: false,
                        score: 0,
                        controller: PlayerController::Network {
                            game_stream_send_channel: game_stream_send_channel_send,
                            connection: connection.connection.clone(),
                        },
                    },
                );

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

        handshake_stream_send
            .send(ni_ty::protocol::HandshakeMessageS2C::Hello)
            .await?;

        let game_stream = connection.connection.open_bi().await?;

        let mut game_stream_send =
            async_bincode::AsyncBincodeWriter::<_, ni_ty::protocol::GameMessageS2C, _>::from(
                game_stream.0,
            )
            .for_async();
        let game_stream_recv = async_bincode::AsyncBincodeReader::<
            _,
            ni_ty::protocol::GameMessageC2S,
        >::from(game_stream.1);

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
                connection.datagrams.map_err(Into::into).try_for_each(|bytes| {
                    let global_state = global_state.clone();
                    async move {
                        use ni_ty::protocol::DatagramMessageC2S;

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

                        Result::<_, anyhow::Error>::Ok(())
                    }
                }).await?;

                Ok(())
            },
            async {
                let global_state = global_state.clone();
                game_stream_recv
                    .map_err(Into::into)
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

                                        maybe_start_hand(&mut server_game_state);
                                    }
                                }
                                GameMessageC2S::ApplyHandAction { action } => {
                                    let mut server_game_state = global_state
                                        .games
                                        .get_mut(&game_id)
                                        .ok_or(anyhow::anyhow!("Unknown game"))?;

                                    if let Some(ref mut hand_state) = server_game_state.hand {
                                        if let Some(player_idx) = hand_state.hand.players().iter().position(|player| player.player_id() == player_id) {
                                            match hand_state.hand.apply(Some(player_idx as u8), action) {
                                                Err(_) => {
                                                    println!("cannot apply action {:?}", action);
                                                }
                                                Ok(_) => {
                                                    send_to_all(&server_game_state, ni_ty::protocol::GameMessageS2C::PlayerHandAction { player: player_idx as u8, action });

                                                    if action.should_reset_stall() {
                                                        let hand_state = server_game_state.hand.as_mut().unwrap();
                                                        hand_state.stalled_count = 0;
                                                        if hand_state.sent_stall {
                                                            hand_state.sent_stall = false;
                                                            send_to_all(&server_game_state, ni_ty::protocol::GameMessageS2C::HandStallCancel);
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

                                    handle_nerts_call(&mut server_game_state, player_id, &global_state);
                                }
                                GameMessageC2S::AddBot => {
                                    let mut server_game_state = global_state
                                        .games
                                        .get_mut(&game_id)
                                        .ok_or(anyhow::anyhow!("Unknown game"))?;

                                    if server_game_state.master_player == Some(player_id) {
                                        let bot_id = loop {
                                            let bot_id = rand::thread_rng().gen();
                                            if !server_game_state.players.contains_key(&bot_id) {
                                                let bot_name = format!("Bot {}", bot_id);

                                                server_game_state.players.insert(
                                                    bot_id,
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

                                        send_to_all(
                                            &server_game_state,
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
                            }
                            Result::<_, anyhow::Error>::Ok(())
                        }
                    })
                    .await?;

                if let Err(_) = leave_send.send(()) {
                    eprintln!("Failed to send leave event");
                }

                Ok(())
            },
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

    maybe_start_hand(&mut server_game_state);

    res
}

#[tokio::main]
async fn main() {
    let port: u16 = std::env::var("PORT")
        .as_deref()
        .unwrap_or("6465")
        .parse()
        .unwrap();

    let (cert, pkey) = {
        let mut keyfile = tempfile::NamedTempFile::new().unwrap();
        let mut certfile = tempfile::NamedTempFile::new().unwrap();

        let status = std::process::Command::new("openssl")
            .args(&[
                "req", "-x509", "-outform", "DER", "-newkey", "rsa:4096", "-keyout",
            ])
            .arg(keyfile.path())
            .arg("-out")
            .arg(certfile.path())
            .args(&["-nodes", "-batch"])
            .status()
            .unwrap();

        if !status.success() {
            panic!("Failed to generate certificate");
        }

        let mut key = Vec::new();
        keyfile.read_to_end(&mut key).unwrap();
        let pkey = openssl::rsa::Rsa::private_key_from_pem(&key).unwrap();
        let pkey = openssl::pkey::PKey::from_rsa(pkey).unwrap();

        let mut cert = Vec::new();
        certfile.read_to_end(&mut cert).unwrap();
        let cert = rustls::Certificate(cert);

        (cert, pkey)
    };

    let privkey = rustls::PrivateKey(pkey.private_key_to_der().unwrap());

    let global_state = Arc::new(GlobalState {
        games: Default::default(),
    });

    let mut transport_config = quinn::TransportConfig::default();
    transport_config.keep_alive_interval(Some(std::time::Duration::from_secs(5)));

    let mut server_config = quinn::ServerConfig::with_single_cert(vec![cert], privkey).unwrap();
    server_config.transport = Arc::new(transport_config);

    let redis_conn_details = match std::env::var("REDIS_URI") {
        Ok(value) => Some((
            std::env::var("MY_HOST_ADDRESS")
                .expect("Missing MY_HOST_ADDRESS")
                .parse()
                .expect("Invalid value for MY_HOST_ADDRESS"),
            {
                let conn = redis_async::client::paired_connect(value)
                    .await
                    .expect("Failed to connnect to Redis");

                let server_id = loop {
                    let server_id: u8 = rand::thread_rng().gen();

                    let res = conn
                        .send::<redis_async::resp::RespValue>(redis_async::resp_array!(
                            "SET",
                            format!("server_ids/{}", server_id),
                            "yes",
                            "EX",
                            "120",
                            "NX",
                        ))
                        .await
                        .expect("Failed to reserve server ID");

                    match res {
                        redis_async::resp::RespValue::Nil => {
                            // try again
                        }
                        redis_async::resp::RespValue::SimpleString(_) => {
                            // success
                            break server_id;
                        }
                        _ => {
                            panic!("Unknown response from server ID reservation: {:?}", res);
                        }
                    }
                };

                (server_id, conn)
            },
        )),
        Err(std::env::VarError::NotPresent) => None,
        Err(std::env::VarError::NotUnicode(_)) => panic!("REDIS_URI is not valid unicode"),
    };

    let (_, incoming) =
        quinn::Endpoint::server(server_config, ([0, 0, 0, 0], port).into()).unwrap();

    futures_util::join!(
        {
            let global_state = global_state.clone();
            incoming.for_each(move |connecting| {
                let global_state = global_state.clone();
                tokio::spawn(async {
                    let res = handle_connection(global_state, connecting).await;
                    if let Err(err) = res {
                        eprintln!("Failed to handle connection: {:?}", err);
                    }
                });

                futures_util::future::ready(())
            })
        },
        {
            let global_state = global_state.clone();

            async move {
                let mut interval = tokio::time::interval(std::time::Duration::from_secs(10));

                loop {
                    interval.tick().await;

                    global_state.games.retain(|_key, value| {
                        !value
                            .players
                            .values()
                            .all(|player| match player.controller {
                                PlayerController::Network { .. } => false,
                                PlayerController::Bot { .. } => true,
                            })
                    });

                    global_state.games.alter_all(|_key, mut value| {
                        if let Some(hand) = value.hand.as_mut() {
                            hand.stalled_count += 1;
                            if hand.sent_stall {
                                use rand::Rng;
                                let seed: u64 = rand::thread_rng().gen();
                                let action = ni_ty::HandAction::ShuffleStock { seed };
                                hand.hand.apply(None, action).unwrap();
                                hand.sent_stall = false;
                                hand.stalled_count = 0;

                                send_to_all(
                                    &value,
                                    ni_ty::protocol::GameMessageS2C::ServerHandAction { action },
                                );
                            } else {
                                if hand.stalled_count >= STALL_SEND_COUNT {
                                    hand.sent_stall = true;
                                    send_to_all(
                                        &value,
                                        ni_ty::protocol::GameMessageS2C::HandStalled,
                                    );
                                }
                            }
                        }
                        value
                    });
                }
            }
        },
        {
            let global_state = global_state.clone();

            async move {
                let mut interval = tokio::time::interval(std::time::Duration::from_millis(500));

                loop {
                    use nertsio_ui_metrics::{
                        CARD_HEIGHT, CARD_WIDTH, HORIZONTAL_STACK_SPACING, VERTICAL_STACK_SPACING,
                    };

                    interval.tick().await;

                    for mut game in global_state.games.iter_mut() {
                        if let Some(hand) = &game.hand {
                            let hand = hand.hand.clone();

                            let metrics = nertsio_ui_metrics::HandMetrics::new(
                                hand.players().len(),
                                hand.players()[0].tableau_stacks().len(),
                                hand.lake_stacks().len(),
                            );

                            for idx in 0..hand.players().len() {
                                let hand_player = &hand.players()[idx];
                                if let Some(player) = game.players.get_mut(&hand_player.player_id())
                                {
                                    let player_loc = metrics.player_loc(idx);

                                    let get_dest_for_stack = |loc, take_count| {
                                        let stack = hand.stack_at(loc).unwrap();
                                        let remaining_count = stack.len() - take_count;

                                        let stack_pos = metrics.stack_pos(loc);

                                        let stack_pos = match loc {
                                            ni_ty::StackLocation::Lake(_) => stack_pos,
                                            ni_ty::StackLocation::Player(_, loc) => match loc {
                                                ni_ty::PlayerStackLocation::Nerts => (
                                                    stack_pos.0
                                                        + (remaining_count as f32)
                                                            * HORIZONTAL_STACK_SPACING,
                                                    stack_pos.1,
                                                ),
                                                ni_ty::PlayerStackLocation::Tableau(_) => (
                                                    stack_pos.0,
                                                    stack_pos.1
                                                        + (remaining_count as f32)
                                                            * VERTICAL_STACK_SPACING,
                                                ),
                                                ni_ty::PlayerStackLocation::Stock => stack_pos,
                                                ni_ty::PlayerStackLocation::Waste => {
                                                    let remaining_visible =
                                                        stack.len().min(3) - take_count;
                                                    (
                                                        stack_pos.0
                                                            + (remaining_visible as f32)
                                                                * HORIZONTAL_STACK_SPACING,
                                                        stack_pos.1,
                                                    )
                                                }
                                            },
                                        };

                                        let stack_pos = if let ni_ty::StackLocation::Lake(_) = loc {
                                            if player_loc.inverted {
                                                (-stack_pos.0 - CARD_WIDTH, stack_pos.1)
                                            } else {
                                                stack_pos
                                            }
                                        } else {
                                            stack_pos
                                        };

                                        (
                                            stack_pos.0 + CARD_WIDTH / 2.0,
                                            stack_pos.1 + CARD_HEIGHT / 2.0,
                                        )
                                    };

                                    let reached = |a: (f32, f32), b: (f32, f32)| {
                                        a.0 > b.0 - CARD_WIDTH / 2.0
                                            && a.0 < b.0 + CARD_WIDTH / 2.0
                                            && a.1 > b.1 - CARD_HEIGHT / 2.0
                                            && a.1 < b.1 + CARD_HEIGHT / 2.0
                                    };

                                    if let PlayerController::Bot {
                                        ref mut plan,
                                        ref mut mouse_state,
                                        ref mut target,
                                        ..
                                    } = &mut player.controller
                                    {
                                        let action = match plan {
                                            None => {
                                                // make a new plan

                                                let mut new_plan = None;

                                                if hand_player.nerts_stack().len() == 0 {
                                                    new_plan = Some(BotPlan::CallNerts);
                                                }

                                                if new_plan.is_none() {
                                                    for src in std::iter::once(
                                                        ni_ty::PlayerStackLocation::Nerts,
                                                    )
                                                    .chain(
                                                        (0..hand_player.tableau_stacks().len())
                                                            .map(|i| {
                                                                ni_ty::PlayerStackLocation::Tableau(
                                                                    i as u8,
                                                                )
                                                            }),
                                                    )
                                                    .chain(std::iter::once(
                                                        ni_ty::PlayerStackLocation::Waste,
                                                    )) {
                                                        let stack =
                                                            hand_player.stack_at(src).unwrap();
                                                        if let Some(card) = stack.last() {
                                                            for (i, stack) in hand
                                                                .lake_stacks()
                                                                .iter()
                                                                .enumerate()
                                                            {
                                                                if stack.can_add(*card) {
                                                                    new_plan = Some(ni_ty::HandAction::Move { from: ni_ty::StackLocation::Player(idx as u8, src), to: ni_ty::StackLocation::Lake(i as u16), count: 1}.into());
                                                                    break;
                                                                }
                                                            }

                                                            match src {
                                                                ni_ty::PlayerStackLocation::Tableau(
                                                                    _,
                                                                )
                                                                | ni_ty::PlayerStackLocation::Nerts => {
                                                                    let src_is_tableau = matches!(src, ni_ty::PlayerStackLocation::Tableau(_));
                                                                    let count = if src_is_tableau {
                                                                        stack.len()
                                                                    } else {
                                                                        1
                                                                    };
                                                                    let back = stack.cards()
                                                                        [stack.len() - count];

                                                                    for (i, dest_stack) in hand_player
                                                                        .tableau_stacks()
                                                                        .iter()
                                                                        .enumerate()
                                                                    {
                                                                        let dest = ni_ty::StackLocation::Player(idx as u8, ni_ty::PlayerStackLocation::Tableau(i as u8));
                                                                        if dest_stack.can_add(back)
                                                                            && (!src_is_tableau
                                                                                || dest_stack.len() > 0)
                                                                        {
                                                                            new_plan = Some(ni_ty::HandAction::Move { from: ni_ty::StackLocation::Player(idx as u8, src), to: dest, count: count as u8 }.into());
                                                                            break;
                                                                        }
                                                                    }
                                                                }
                                                                _ => {}
                                                            }
                                                        }
                                                    }
                                                }

                                                if new_plan.is_none() {
                                                    if hand_player.stock_stack().len() > 0 {
                                                        new_plan = Some(
                                                            ni_ty::HandAction::FlipStock.into(),
                                                        );
                                                    } else if hand_player.waste_stack().len() > 0 {
                                                        new_plan = Some(
                                                            ni_ty::HandAction::ReturnStock.into(),
                                                        );
                                                    }
                                                }

                                                if let Some(new_plan) = new_plan {
                                                    if let Some(held) = mouse_state.held {
                                                        if match new_plan {
                                                            BotPlan::CallNerts => true,
                                                            BotPlan::Action(action) => match action {
                                                                ni_ty::HandAction::ShuffleStock {
                                                                    ..
                                                                } => unreachable!(),
                                                                ni_ty::HandAction::FlipStock
                                                                | ni_ty::HandAction::ReturnStock => {
                                                                    true
                                                                }
                                                                ni_ty::HandAction::Move {
                                                                    from,
                                                                    count,
                                                                    ..
                                                                } => {
                                                                    ni_ty::StackLocation::Player(
                                                                        idx as u8, held.src,
                                                                    ) != from
                                                                        || held.count != count
                                                                }
                                                            }
                                                        } {
                                                            mouse_state.held = None;
                                                        }
                                                    }

                                                    *plan = Some(new_plan);
                                                }

                                                None
                                            }
                                            Some(current_plan) => {
                                                let current_plan = current_plan.clone();
                                                match current_plan {
                                                    BotPlan::CallNerts => {
                                                        let dest = get_dest_for_stack(
                                                            ni_ty::StackLocation::Player(
                                                                idx as u8,
                                                                ni_ty::PlayerStackLocation::Nerts,
                                                            ),
                                                            0,
                                                        );
                                                        if reached(mouse_state.position, dest) {
                                                            *plan = None;

                                                            Some(current_plan)
                                                        } else {
                                                            *target = dest;

                                                            None
                                                        }
                                                    }
                                                    BotPlan::Action(action) => match action {
                                                        ni_ty::HandAction::ShuffleStock {
                                                            ..
                                                        } => {
                                                            unreachable!()
                                                        }
                                                        ni_ty::HandAction::Move {
                                                            from,
                                                            to,
                                                            count,
                                                        } => {
                                                            if mouse_state.held.is_some() {
                                                                let dest =
                                                                    get_dest_for_stack(to, 0);
                                                                if reached(
                                                                    mouse_state.position,
                                                                    dest,
                                                                ) {
                                                                    *plan = None;

                                                                    Some(current_plan)
                                                                } else {
                                                                    *target = dest;
                                                                    None
                                                                }
                                                            } else {
                                                                let dest = get_dest_for_stack(
                                                                    from,
                                                                    count.into(),
                                                                );
                                                                if reached(
                                                                    mouse_state.position,
                                                                    dest,
                                                                ) {
                                                                    let from_stack = hand
                                                                        .stack_at(from)
                                                                        .unwrap();
                                                                    if from_stack.len()
                                                                        >= count.into()
                                                                    {
                                                                        mouse_state.held = Some(ni_ty::HeldInfo {
                                                                        src: if let ni_ty::StackLocation::Player(_, loc) = from {
                                                                            loc
                                                                        } else {
                                                                            panic!("somehow picked up a non-player stack")
                                                                        },
                                                                        count,
                                                                        offset: (
                                                                            mouse_state.position.0 - (dest.0 - CARD_WIDTH / 2.0),
                                                                            mouse_state.position.1 - (dest.1 - CARD_HEIGHT / 2.0),
                                                                        ),
                                                                        top_card: from_stack.cards()[from_stack.len() - usize::from(count)].card,
                                                                    });

                                                                        *target =
                                                                            get_dest_for_stack(
                                                                                to, 0,
                                                                            );
                                                                    } else {
                                                                        *plan = None;
                                                                    }
                                                                } else {
                                                                    *target = dest;
                                                                }

                                                                None
                                                            }
                                                        }
                                                        ni_ty::HandAction::FlipStock
                                                        | ni_ty::HandAction::ReturnStock => {
                                                            let dest = get_dest_for_stack(
                                                                ni_ty::StackLocation::Player(
                                                                    idx as u8,
                                                                    ni_ty::PlayerStackLocation::Stock,
                                                                ),
                                                                0,
                                                            );

                                                            if reached(mouse_state.position, dest) {
                                                                *plan = None;

                                                                Some(current_plan)
                                                            } else {
                                                                *target = dest;

                                                                None
                                                            }
                                                        }
                                                    },
                                                }
                                            }
                                        };

                                        if let Some(action) = action {
                                            match action {
                                                BotPlan::CallNerts => {
                                                    handle_nerts_call(
                                                        &mut game,
                                                        hand_player.player_id(),
                                                        &global_state,
                                                    );
                                                }
                                                BotPlan::Action(action) => {
                                                    if game
                                                        .hand
                                                        .as_mut()
                                                        .unwrap()
                                                        .hand
                                                        .apply(Some(idx as u8), action)
                                                        .is_ok()
                                                    {
                                                        send_to_all(
                                                            &game,
                                                            ni_ty::protocol::GameMessageS2C::PlayerHandAction {
                                                                player: idx as u8,
                                                                action,
                                                            },
                                                        );
                                                        if action.should_reset_stall() {
                                                            let hand_state =
                                                                game.hand.as_mut().unwrap();
                                                            hand_state.stalled_count = 0;
                                                            if hand_state.sent_stall {
                                                                hand_state.sent_stall = false;
                                                                send_to_all(&game, ni_ty::protocol::GameMessageS2C::HandStallCancel);
                                                            }
                                                        }

                                                        let hand_player = &hand.players()[idx];
                                                        if let Some(player) = game
                                                            .players
                                                            .get_mut(&hand_player.player_id())
                                                        {
                                                            if let PlayerController::Bot {
                                                                ref mut mouse_state,
                                                                ..
                                                            } = &mut player.controller
                                                            {
                                                                mouse_state.held = None;
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
                    }
                }
            }
        },
        {
            let global_state = global_state.clone();

            async move {
                let mut interval = tokio::time::interval(std::time::Duration::from_millis(50));

                loop {
                    interval.tick().await;

                    for mut game in global_state.games.iter_mut() {
                        if let Some(hand) = &game.hand {
                            let player_count = hand.hand.players().len();

                            for idx in 0..player_count {
                                let player_id =
                                    game.hand.as_ref().unwrap().hand.players()[idx].player_id();
                                if let Some(player) = game.players.get_mut(&player_id) {
                                    if let PlayerController::Bot {
                                        ref mut mouse_state,
                                        ref mut target,
                                        ref mut seq,
                                        ref plan,
                                    } = &mut player.controller
                                    {
                                        if plan.is_some() {
                                            let dist = ((mouse_state.position.0 - target.0)
                                                .powf(2.0)
                                                + (mouse_state.position.1 - target.1).powf(2.0))
                                            .sqrt();

                                            if dist > BOT_CURSOR_SPEED {
                                                mouse_state.position = (
                                                    mouse_state.position.0
                                                        + (target.0 - mouse_state.position.0)
                                                            / dist
                                                            * BOT_CURSOR_SPEED,
                                                    mouse_state.position.1
                                                        + (target.1 - mouse_state.position.1)
                                                            / dist
                                                            * BOT_CURSOR_SPEED,
                                                );

                                                *seq += 1;

                                                let out_msg: bytes::Bytes = bincode::serialize(&ni_ty::protocol::DatagramMessageS2C::UpdateMouseState {
                                                    player_idx: idx as u8,
                                                    seq: *seq,
                                                    state: mouse_state.clone(),
                                                }).unwrap().into();

                                                for (id, server_player_state) in &game.players {
                                                    if *id != player_id {
                                                        if let PlayerController::Network {
                                                            ref connection,
                                                            ..
                                                        } = server_player_state.controller
                                                        {
                                                            if let Err(err) = connection
                                                                .send_datagram(out_msg.clone())
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
                    }
                }
            }
        },
        {
            let redis_conn_details = redis_conn_details.clone();
            async move {
                if let Some((my_address_ipv4, (server_id, redis_conn))) = redis_conn_details {
                    let mut interval = tokio::time::interval(std::time::Duration::from_secs(2));

                    loop {
                        interval.tick().await;

                        let status = ni_ty::protocol::ServerStatusMessage {
                            server_id,
                            address_ipv4: my_address_ipv4,
                            min_protocol_version: ni_ty::protocol::PROTOCOL_VERSION,
                            protocol_version: ni_ty::protocol::PROTOCOL_VERSION,
                            open_public_games: global_state
                                .games
                                .iter()
                                .filter(|entry| {
                                    entry.value().public
                                        && entry.value().players.len() < MAX_PLAYERS
                                })
                                .map(|entry| ni_ty::protocol::PublicGameInfo {
                                    game_id: *entry.key(),
                                    players: entry.value().players.len() as u8,
                                    waiting: entry.value().hand.is_none(),
                                })
                                .collect(),
                            stats: global_state.games.iter().fold(
                                ni_ty::protocol::ServerStats {
                                    public_games: 0,
                                    private_games: 0,
                                    public_game_players: 0,
                                    private_game_players: 0,
                                },
                                |mut acc, entry| {
                                    if entry.value().public {
                                        acc.public_games += 1;
                                        acc.public_game_players +=
                                            entry.value().players.len() as u32;
                                    } else {
                                        acc.private_games += 1;
                                        acc.private_game_players +=
                                            entry.value().players.len() as u32;
                                    }

                                    acc
                                },
                            ),
                        };

                        if let Err(err) = redis_conn
                            .send::<i64>(redis_async::resp_array!(
                                "PUBLISH",
                                ni_ty::protocol::COORDINATOR_CHANNEL,
                                bincode::serialize(&status).unwrap()
                            ))
                            .await
                        {
                            eprintln!("failed to publish status: {:?}", err);
                        }
                    }
                }
            }
        },
        async move {
            if let Some((_, (server_id, redis_conn))) = &redis_conn_details {
                let mut interval = tokio::time::interval(std::time::Duration::from_secs(100));

                loop {
                    interval.tick().await;

                    if let Err(err) = redis_conn
                        .send::<String>(redis_async::resp_array!(
                            "SET",
                            format!("server_ids/{}", server_id),
                            "yes",
                            "EX",
                            "120",
                        ))
                        .await
                    {
                        eprintln!("failed to renew ID reservation: {:?}", err);
                    }
                }
            }
        }
    );
}
