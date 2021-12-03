use futures_util::sink::SinkExt;
use futures_util::stream::{StreamExt, TryStreamExt};
use nertsio_types as ni_ty;
use rand::Rng;
use std::collections::HashMap;
use std::io::Read;
use std::sync::Arc;

struct ServerGamePlayerState {
    name: String,
    game_stream_send_channel: tokio::sync::mpsc::UnboundedSender<ni_ty::protocol::GameMessageS2C>,
    ready: bool,
}

impl ServerGamePlayerState {
    pub fn to_common_state(&self) -> ni_ty::GamePlayerState {
        ni_ty::GamePlayerState {
            name: self.name.clone(),
            ready: self.ready,
        }
    }
}

struct ServerGameState {
    players: HashMap<u8, ServerGamePlayerState>,
}

struct GlobalState {
    games: dashmap::DashMap<u32, ServerGameState>,
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
    let (name, game_id) =
        if let ni_ty::protocol::HandshakeMessageC2S::Hello { name, game_id } = first_message {
            (name, game_id)
        } else {
            anyhow::bail!("Wrong first handshake message");
        };

    let (game_stream_send_channel_send, mut game_stream_send_channel_recv) =
        tokio::sync::mpsc::unbounded_channel();

    let (player_id, game_state) = {
        let mut server_game_state = global_state
            .games
            .get_mut(&game_id)
            .ok_or(anyhow::anyhow!("Unknown game"))?;

        let player_id = loop {
            let player_id = rand::thread_rng().gen();
            if !server_game_state.players.contains_key(&player_id) {
                server_game_state.players.insert(
                    player_id,
                    ServerGamePlayerState {
                        name,
                        game_stream_send_channel: game_stream_send_channel_send,
                        ready: false,
                    },
                );

                break player_id;
            }
        };

        let game_state = ni_ty::GameState {
            players: server_game_state
                .players
                .iter()
                .map(|(key, value)| (*key, value.to_common_state()))
                .collect(),
        };

        (player_id, game_state)
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

        futures_util::future::try_join(
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
        )
        .await?;

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
    global_state.games.insert(
        42,
        ServerGameState {
            players: Default::default(),
        },
    );

    let mut transport_config = quinn::TransportConfig::default();
    transport_config.keep_alive_interval(Some(std::time::Duration::from_secs(5)));

    let mut server_config = quinn::ServerConfig::with_single_cert(vec![cert], privkey).unwrap();
    server_config.transport = Arc::new(transport_config);

    let (_, incoming) =
        quinn::Endpoint::server(server_config, ([0, 0, 0, 0], port).into()).unwrap();

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
