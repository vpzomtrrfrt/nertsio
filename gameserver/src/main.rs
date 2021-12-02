use futures_util::sink::SinkExt;
use futures_util::stream::StreamExt;
use nertsio_types as ni_ty;
use rand::Rng;
use std::collections::HashMap;
use std::sync::Arc;

struct ServerGamePlayerState {
    name: String,
    game_stream_send_channel: tokio::sync::mpsc::UnboundedSender<ni_ty::protocol::GameMessageS2C>,
    ready: bool,
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
                .map(|(key, value)| {
                    (
                        *key,
                        ni_ty::GamePlayerState {
                            name: value.name.clone(),
                            ready: value.ready,
                        },
                    )
                })
                .collect(),
        };

        (player_id, game_state)
    };

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

    while let Some(msg) = game_stream_send_channel_recv.recv().await {
        game_stream_send.send(msg).await?;
    }

    Ok(())
}

#[tokio::main]
async fn main() {
    let port: u16 = std::env::var("PORT")
        .as_deref()
        .unwrap_or("6465")
        .parse()
        .unwrap();

    let keypair = openssl::rsa::Rsa::generate(2048).unwrap();
    let pkey = openssl::pkey::PKey::from_rsa(keypair).unwrap();

    let cert = rustls::Certificate(
        {
            let mut builder = openssl::x509::X509Builder::new().unwrap();
            builder.set_pubkey(&pkey).unwrap();
            builder.build()
        }
        .to_der()
        .unwrap(),
    );
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
