#![allow(clippy::collapsible_else_if)]
#![allow(clippy::collapsible_if)]
#![allow(clippy::manual_map)]

use nertsio_types as ni_ty;
use rand::Rng;
use redis::AsyncCommands;
use std::collections::HashMap;
use std::io::Read;
use std::sync::Arc;

mod connection;
mod systems;

const MAX_PLAYERS: usize = 6;
const WIN_SCORE: i32 = 100;
const MIN_PROTOCOL_VERSION: u16 = 10;

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
        connection: Box<dyn connection::ConnectionHandle + Send + Sync>,
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
    spectating: bool,
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
            spectating: self.spectating,
        }
    }
}

struct ServerGameState {
    game_id: u32,
    players: HashMap<u8, ServerGamePlayerState>,
    hand: Option<ServerHandState>,
    public: bool,
    master_player: Option<u8>,
    settings: ni_ty::GameSettings,
}

impl ServerGameState {
    pub fn new(game_id: u32, public: bool) -> Self {
        Self {
            game_id,
            players: Default::default(),
            hand: None,
            public,
            master_player: None,
            settings: Default::default(),
        }
    }

    pub fn send_to_all(&self, msg: ni_ty::protocol::GameMessageS2C) {
        for (id, server_player_state) in &self.players {
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

    pub fn handle_nerts_call(&mut self, player_id: u8, global_state: &Arc<GlobalState>) {
        let send_to_others =
            move |server_game_state: &ServerGameState, msg: ni_ty::protocol::GameMessageS2C| {
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

        let game_id = self.game_id;

        if let Some(ref mut hand_state) = self.hand {
            if let Some(player_idx) = hand_state
                .hand
                .players()
                .iter()
                .position(|player| player.player_id() == player_id)
            {
                if hand_state.hand.players()[player_idx]
                    .nerts_stack()
                    .is_empty()
                    && !hand_state.hand.nerts_called
                {
                    hand_state.hand.nerts_called = true;
                    send_to_others(
                        self,
                        ni_ty::protocol::GameMessageS2C::NertsCalled {
                            player: player_idx as u8,
                        },
                    );
                    let _ = self;

                    let global_state = global_state.clone();
                    tokio::spawn(async move {
                        tokio::time::sleep(std::time::Duration::from_secs(1)).await;

                        if let Some(mut server_game_state) = global_state.games.get_mut(&game_id) {
                            if let Some(hand_state) = server_game_state.hand.take() {
                                let mut scores: Vec<_> = hand_state
                                    .hand
                                    .players()
                                    .iter()
                                    .map(|player| {
                                        -1 * (player.nerts_stack().len() as i32)
                                            * (server_game_state.settings.nerts_card_penalty as i32)
                                    })
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

                                server_game_state.send_to_all(
                                    ni_ty::protocol::GameMessageS2C::HandEnd { scores },
                                );

                                if now_won {
                                    for player in server_game_state.players.values_mut() {
                                        player.score = 0;
                                    }

                                    server_game_state
                                        .send_to_all(ni_ty::protocol::GameMessageS2C::GameEnd);
                                }

                                let mut bots_ready = Vec::new();
                                for (key, player) in server_game_state.players.iter_mut() {
                                    if let PlayerController::Bot { ref mut plan, .. } =
                                        player.controller
                                    {
                                        player.ready = true;
                                        bots_ready.push(*key);
                                        *plan = None;
                                    }
                                }

                                for id in bots_ready {
                                    server_game_state.send_to_all(
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
}

struct GlobalState {
    games: dashmap::DashMap<u32, ServerGameState>,
}

#[tokio::main]
async fn main() {
    let web_port: u16 = std::env::var("WEB_PORT")
        .as_deref()
        .unwrap_or("6466")
        .parse()
        .unwrap();

    let my_region = match std::env::var("MY_REGION") {
        Ok(value) => Some(value),
        Err(std::env::VarError::NotPresent) => None,
        Err(std::env::VarError::NotUnicode(_)) => panic!("MY_REGION is not valid unicode"),
    };

    let (certs, pkey) = match std::env::var_os("CERTIFICATE_FILE") {
        Some(certfile) => {
            let keyfile =
                std::env::var_os("CERTIFICATE_KEY_FILE").expect("Missing CERTIFICATE_KEY_FILE");

            let certfile = std::fs::File::open(certfile).expect("Failed to open CERTIFICATE_FILE");
            let mut keyfile =
                std::fs::File::open(keyfile).expect("Failed to open CERTIFICATE_KEY_FILE");

            let mut key = Vec::new();
            keyfile.read_to_end(&mut key).unwrap();
            let pkey = openssl::pkey::PKey::private_key_from_pem(&key).unwrap();

            let mut certfile = std::io::BufReader::new(certfile);

            let certs = rustls_pemfile::certs(&mut certfile)
                .expect("Failed to parse certificate")
                .into_iter()
                .map(rustls::Certificate)
                .collect();

            (certs, pkey)
        }
        None => {
            let mut keyfile = tempfile::NamedTempFile::new().unwrap();
            let mut certfile = tempfile::NamedTempFile::new().unwrap();

            let status = std::process::Command::new("openssl")
                .args([
                    "req", "-x509", "-outform", "DER", "-newkey", "rsa:4096", "-keyout",
                ])
                .arg(keyfile.path())
                .arg("-out")
                .arg(certfile.path())
                .args(["-nodes", "-batch"])
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

            (vec![cert], pkey)
        }
    };

    let privkey = rustls::PrivateKey(pkey.private_key_to_der().unwrap());

    let global_state = Arc::new(GlobalState {
        games: Default::default(),
    });

    let mut server_config = rustls::ServerConfig::builder()
        .with_safe_default_cipher_suites()
        .with_safe_default_kx_groups()
        .with_protocol_versions(&[&rustls::version::TLS13])
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(certs, privkey)
        .expect("Failed to initialize TLS config");

    server_config.key_log = Arc::new(rustls::KeyLogFile::new());

    let server_config = Arc::new(server_config);

    let web_server_config = {
        let mut config = (*server_config).clone();
        config
            .alpn_protocols
            .push(webtransport_quinn::ALPN.to_vec());

        Arc::new(config)
    };

    let mut transport_config = quinn::TransportConfig::default();
    transport_config.keep_alive_interval(Some(std::time::Duration::from_secs(5)));

    let mut quic_server_config = quinn::ServerConfig::with_crypto(server_config);
    quic_server_config.transport = Arc::new(transport_config);

    let mut web_quic_server_config = quinn::ServerConfig::with_crypto(web_server_config);
    web_quic_server_config.transport = quic_server_config.transport.clone();

    let redis_conn_details = match std::env::var("REDIS_URI") {
        Ok(value) => Some((
            std::env::var("MY_HOSTNAME")
                .expect("Missing MY_HOSTNAME")
                .parse()
                .expect("Invalid value for MY_HOSTNAME"),
            {
                let mut conn = redis::Client::open(value)
                    .expect("Failed to connect to Redis")
                    .get_multiplexed_async_connection()
                    .await
                    .expect("Failed to connnect to Redis");

                let server_id = loop {
                    let server_id: u8 = rand::thread_rng().gen();

                    let res = conn
                        .set_options(
                            format!("server_ids/{}", server_id),
                            "yes",
                            redis::SetOptions::default()
                                .with_expiration(redis::SetExpiry::EX(120))
                                .conditional_set(redis::ExistenceCheck::NX),
                        )
                        .await
                        .expect("Failed to reserve server ID");

                    match res {
                        redis::Value::Nil => {
                            // try again
                        }
                        redis::Value::Okay => {
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

    let web_quic_endpoint =
        quinn::Endpoint::server(web_quic_server_config, ([0, 0, 0, 0], web_port).into()).unwrap();
    let web_incoming = futures_util::stream::unfold((), |()| async {
        if let Some(connecting) = web_quic_endpoint.accept().await {
            Some((
                async {
                    let conn = connecting.await?;

                    let req = webtransport_quinn::accept(conn).await?;
                    let session = req.ok().await?;

                    Result::<_, anyhow::Error>::Ok(session)
                },
                (),
            ))
        } else {
            None
        }
    });

    futures_util::join!(
        systems::handle_connection::run(global_state.clone(), web_incoming),
        systems::cleanup::run(global_state.clone()),
        {
            let global_state = global_state.clone();
            let redis_conn_details = redis_conn_details.clone();
            async move {
                if let Some((my_hostname, (server_id, redis_conn))) = redis_conn_details {
                    systems::publish::run(
                        global_state,
                        my_hostname,
                        web_port,
                        server_id,
                        my_region,
                        redis_conn,
                    )
                    .await;
                }
            }
        },
        systems::bots::run(global_state),
    );
}
