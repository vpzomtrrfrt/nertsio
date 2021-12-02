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
        );
    let handshake_stream_recv = async_bincode::AsyncBincodeReader::<
        _,
        ni_ty::protocol::HandshakeMessageC2S,
    >::from(handshake_stream.1);

    let (first_message, _handshake_stream_recv) = handshake_stream_recv.into_future().await;
    let first_message = first_message.ok_or(anyhow::anyhow!("Stream closed without Hello"))??;

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
                if let Err(err) = server_player_state
                    .game_stream_send_channel
                    .send(msg.clone())
                {
                    eprintln!("Failed to queue update to player: {:?}", err);
                }
            }
        }
    };

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
        .send(ni_ty::protocol::HandshakeMessageS2C::Joined {
            info: game_state,
            your_player_id: player_id,
        })
        .await?;

    let game_stream = connection.connection.open_bi().await?;
    let mut game_stream_send =
        async_bincode::AsyncBincodeWriter::<_, ni_ty::protocol::GameMessageS2C, _>::from(
            game_stream.0,
        );
    let game_stream_recv =
        async_bincode::AsyncBincodeReader::<_, ni_ty::protocol::GameMessageC2S>::from(
            game_stream.1,
        );

    futures_util::future::try_join(
        async {
            while let Some(msg) = game_stream_send_channel_recv.recv().await {
                game_stream_send.send(msg).await?;
            }
            Result::<_, anyhow::Error>::Ok(())
        },
        game_stream_recv
            .map_err(Into::into)
            .try_for_each(move |msg| {
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
                    Ok(())
                }
            }),
    )
    .await?;

    Ok(())
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
            .args(&[
                "-nodes",
                "-batch",
                "-subj",
                "/",
                "-addext",
                "basicConstraints=CA:FALSE",
            ])
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

    let (_, incoming) = quinn::Endpoint::server(
        quinn::ServerConfig::with_single_cert(vec![cert], privkey).unwrap(),
        ([0, 0, 0, 0], port).into(),
    )
    .unwrap();

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
