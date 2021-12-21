use futures_util::sink::SinkExt;
use futures_util::stream::{StreamExt, TryStreamExt};
use nertsio_types as ni_ty;
use rand::Rng;
use std::collections::HashMap;
use std::io::Read;
use std::sync::Arc;

const MAX_PLAYERS: usize = 6;

struct ServerGamePlayerState {
    name: String,
    game_stream_send_channel: tokio::sync::mpsc::UnboundedSender<ni_ty::protocol::GameMessageS2C>,
    ready: bool,
    score: i32,
    connection: quinn::Connection,
}

struct ServerHandState {
    hand: ni_ty::HandState,
    mouse_states: Vec<Option<(u32, ni_ty::MouseState)>>,
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
    players: HashMap<u8, ServerGamePlayerState>,
    hand: Option<ServerHandState>,
    public: bool,
}

impl ServerGameState {
    pub fn new(public: bool) -> Self {
        Self {
            players: Default::default(),
            hand: None,
            public,
        }
    }
}

struct GlobalState {
    games: dashmap::DashMap<u32, ServerGameState>,
}

fn send_to_all(server_game_state: &ServerGameState, msg: ni_ty::protocol::GameMessageS2C) {
    for (id, server_player_state) in &server_game_state.players {
        println!("sending {:?} to {}", msg, id);
        if let Err(err) = server_player_state
            .game_stream_send_channel
            .send(msg.clone())
        {
            eprintln!("Failed to queue update to player: {:?}", err);
        }
    }
}

fn maybe_start_hand(server_game_state: &mut ServerGameState) {
    if server_game_state
        .players
        .values()
        .all(|player| player.ready)
    {
        // all ready, start hand

        let new_hand = ni_ty::HandState::generate(server_game_state.players.keys().copied());
        server_game_state.hand = Some(ServerHandState {
            hand: new_hand.clone(),
            mouse_states: vec![None; new_hand.players().len()],
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
                    let game_id: u32 = rand::thread_rng().gen();
                    if let dashmap::mapref::entry::Entry::Vacant(entry) =
                        global_state.games.entry(game_id)
                    {
                        break (entry.insert(ServerGameState::new(new_game_public)), game_id);
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
                        game_stream_send_channel: game_stream_send_channel_send,
                        ready: false,
                        score: 0,
                        connection: connection.connection.clone(),
                    },
                );

                break player_id;
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
        };

        (player_id, game_state, game_id)
    };

    let send_to_others = move |server_game_state: &ServerGameState,
                               msg: ni_ty::protocol::GameMessageS2C| {
        for (id, server_player_state) in &server_game_state.players {
            if *id != player_id {
                println!("sending {:?} to {}", msg, id);
                if let Err(err) = server_player_state
                    .game_stream_send_channel
                    .send(msg.clone())
                {
                    eprintln!("Failed to queue update to player: {:?}", err);
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
                                                    if let Err(err) = server_player_state.connection.send_datagram(out_msg.clone())
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
                                            match hand_state.hand.apply(player_idx as u8, action) {
                                                Err(_) => {
                                                    println!("cannot apply action {:?}", action);
                                                }
                                                Ok(_) => {
                                                    send_to_all(&server_game_state, ni_ty::protocol::GameMessageS2C::PlayerHandAction { player: player_idx as u8, action });
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

                                    if let Some(ref mut hand_state) = server_game_state.hand {
                                        if let Some(player_idx) = hand_state.hand.players().iter().position(|player| player.player_id() == player_id) {
                                            if hand_state.hand.players()[player_idx].nerts_stack().len() == 0 {
                                                hand_state.hand.nerts_called = true;
                                                send_to_others(&server_game_state, ni_ty::protocol::GameMessageS2C::NertsCalled { player: player_idx as u8 });
                                                let _ = server_game_state;

                                                let global_state = global_state.clone();
                                                tokio::spawn(async move {
                                                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

                                                    if let Some(mut server_game_state) = global_state.games.get_mut(&game_id) {
                                                        if let Some(hand_state) = server_game_state.hand.take() {
                                                            let mut scores: Vec<_> = hand_state.hand.players().iter().map(|player| (player.nerts_stack().len() as i32) * (-2)).collect();
                                                            for stack in hand_state.hand.lake_stacks() {
                                                                for card in stack.cards() {
                                                                    scores[card.owner_id as usize] += 1;
                                                                }
                                                            }

                                                            hand_state.hand.players().iter().zip(scores.iter()).for_each(|(player, score)| {
                                                                if let Some(info) = server_game_state.players.get_mut(&player.player_id()) {
                                                                    info.score += score;
                                                                }
                                                            });

                                                            for player in server_game_state.players.values_mut() {
                                                                player.ready = false;
                                                            }

                                                            send_to_all(&server_game_state, ni_ty::protocol::GameMessageS2C::HandEnd { scores });
                                                        }
                                                    }
                                                });
                                            }
                                        }
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

                    global_state
                        .games
                        .retain(|_key, value| !value.players.is_empty())
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
