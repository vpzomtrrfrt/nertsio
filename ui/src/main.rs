use futures_util::{SinkExt, StreamExt, TryStreamExt};
use macroquad::prelude as mq;
use macroquad::ui as mqui;
use nertsio_types as ni_ty;
use std::collections::VecDeque;
use std::sync::Arc;

const BACKGROUND_COLOR: mq::Color = mq::Color::new(0.2, 0.7, 0.2, 1.0);

const CARD_WIDTH: f32 = 90.0;
const CARD_HEIGHT: f32 = 135.0;
const LAKE_SPACING: f32 = 10.0;
const HORIZONTAL_STACK_SPACING: f32 = 10.0;
const VERTICAL_STACK_SPACING: f32 = 20.0;
const PLAYER_SPACING: f32 = 20.0;
const PLAYER_Y: f32 = 200.0;
const OTHER_CURSOR_SIZE: f32 = 4.0;

const GAME_ID_FORMAT: u128 = lexical::NumberFormatBuilder::from_radix(36);

const COORDINATOR_URL: &str = "http://localhost:6462/";

struct SharedInfo {
    game: ni_ty::GameState,
    my_player_id: u8,
    pending_actions: VecDeque<ni_ty::HandAction>,
    self_called_nerts: bool,
    hand_mouse_states: Option<Vec<Option<(u32, ni_ty::MouseState)>>>,
    my_held_state: Option<ni_ty::HeldInfo>,
    last_mouse_position: Option<(f32, f32)>,
    server_id: u8,
}

enum State {
    MainMenu,
    JoinGameForm { input: String },
    Connecting,
    GameNeutral,
    GameHand { my_player_idx: usize },
}

fn parse_full_game_id_str(src: &str) -> Result<(u8, u32), lexical::Error> {
    let result: u64 = lexical::parse_with_options::<_, _, GAME_ID_FORMAT>(
        &src,
        &lexical::parse_integer_options::Options::default(),
    )?;

    Ok(((result >> 32) as u8, (result & (u32::MAX as u64)) as u32))
}

fn to_full_game_id_str(server_id: u8, game_id: u32) -> String {
    lexical::to_string_with_options::<_, GAME_ID_FORMAT>(
        u64::from(game_id) + (u64::from(server_id) << 32),
        &lexical::write_integer_options::Options::default(),
    )
}

fn get_card_rect(card: ni_ty::Card) -> mq::Rect {
    const SPACING: f32 = 10.0;
    const WIDTH: f32 = 120.0;
    const HEIGHT: f32 = 180.0;

    let x = SPACING + (f32::from(card.rank.value() - 1) * (WIDTH + SPACING));
    let y = SPACING
        + ((match card.suit {
            ni_ty::Suit::Spades => 0.0,
            ni_ty::Suit::Hearts => 1.0,
            ni_ty::Suit::Diamonds => 2.0,
            ni_ty::Suit::Clubs => 3.0,
        }) * (HEIGHT + SPACING));

    mq::Rect {
        x,
        y,
        w: WIDTH,
        h: HEIGHT,
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

enum ConnectionType {
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

pub async fn res_to_error(
    res: hyper::Response<hyper::Body>,
) -> Result<hyper::Response<hyper::Body>, anyhow::Error> {
    let status = res.status();
    if status.is_success() {
        Ok(res)
    } else {
        let bytes = hyper::body::to_bytes(res.into_body()).await?;
        Err(anyhow::anyhow!(
            "Remote error {}: {}",
            status,
            String::from_utf8_lossy(&bytes)
        ))
    }
}

async fn handle_connection<C: hyper::client::connect::Connect + Clone + Send + Sync + 'static>(
    http_client: &hyper::Client<C>,
    connection_type: ConnectionType,
    info_mutex: &std::sync::Mutex<Option<SharedInfo>>,
    mut game_msg_recv: tokio::sync::mpsc::UnboundedReceiver<ni_ty::protocol::GameMessageC2S>,
) -> Result<(), anyhow::Error> {
    let (server, game_id, new_game_public) = match connection_type {
        ConnectionType::CreateGame { public } => {
            let resp = res_to_error(
                http_client
                    .request(
                        hyper::Request::post(format!(
                            "{}servers:pick_for_new_game",
                            COORDINATOR_URL
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
                        hyper::Request::get(format!("{}servers/{}", COORDINATOR_URL, server_id))
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
        name: "Nerter".to_owned(),
        game_id,
        new_game_public,
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

    futures_util::try_join!(
        async {
            while let Some(msg) = game_msg_recv.recv().await {
                println!("sending {:?}", msg);
                game_stream_send.send(msg).await?;
            }
            Result::<_, anyhow::Error>::Ok(())
        },
        async {
            let mut interval = tokio::time::interval(std::time::Duration::from_millis(50));
            let mut seq: u32 = 0;

            loop {
                interval.tick().await;

                let mut lock = info_mutex.lock().unwrap();
                let shared = lock.as_mut().unwrap();

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
                            let shared = (*lock).as_mut().unwrap();

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
        async {
            game_stream_recv
                .map_err(Into::into)
                .try_for_each(|msg| async {
                    use ni_ty::protocol::GameMessageS2C;

                    println!("received {:?}", msg);

                    match msg {
                        GameMessageS2C::Joined {
                            info,
                            your_player_id,
                        } => {
                            *info_mutex.lock().unwrap() = Some(SharedInfo {
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
                                server_id: server.server_id,
                            });
                        }
                        GameMessageS2C::PlayerJoin { id, info } => {
                            (*info_mutex.lock().unwrap())
                                .as_mut()
                                .unwrap()
                                .game
                                .players
                                .insert(id, info);
                        }
                        GameMessageS2C::PlayerLeave { id } => {
                            (*info_mutex.lock().unwrap())
                                .as_mut()
                                .unwrap()
                                .game
                                .players
                                .remove(&id);
                        }
                        GameMessageS2C::PlayerUpdateReady { id, value } => {
                            (*info_mutex.lock().unwrap())
                                .as_mut()
                                .unwrap()
                                .game
                                .players
                                .get_mut(&id)
                                .unwrap()
                                .ready = value;
                        }
                        GameMessageS2C::HandStart { info } => {
                            let mut lock = info_mutex.lock().unwrap();
                            let shared = (*lock).as_mut().unwrap();

                            shared.hand_mouse_states = Some(vec![None; info.players().len()]);
                            shared.game.hand = Some(info);
                            shared.my_held_state = None;
                        }
                        GameMessageS2C::PlayerHandAction { player, action } => {
                            let mut lock = info_mutex.lock().unwrap();
                            let shared = lock.as_mut().unwrap();

                            let hand = shared.game.hand.as_mut().unwrap();

                            let my_player_idx = hand
                                .players()
                                .iter()
                                .position(|player| player.player_id() == shared.my_player_id)
                                .unwrap();

                            if player == my_player_idx as u8 {
                                // my move, check if matches expected

                                while let Some(front) = shared.pending_actions.pop_front() {
                                    if front == action {
                                        break;
                                    }
                                }
                            }

                            hand.apply(player, action).unwrap();
                        }
                        GameMessageS2C::NertsCalled { player: _ } => {
                            let mut lock = info_mutex.lock().unwrap();
                            let shared = lock.as_mut().unwrap();

                            let hand = shared.game.hand.as_mut().unwrap();

                            hand.nerts_called = true;
                        }
                        GameMessageS2C::HandEnd { scores } => {
                            let mut lock = info_mutex.lock().unwrap();
                            let shared = lock.as_mut().unwrap();

                            let hand_state = shared.game.hand.take().unwrap();

                            for (player, score) in hand_state.players().iter().zip(scores) {
                                if let Some(info) = shared.game.players.get_mut(&player.player_id())
                                {
                                    info.score += score;
                                }
                            }

                            for player in shared.game.players.values_mut() {
                                player.ready = false;
                            }
                        }
                    }

                    Ok(())
                })
                .await
        },
    )?;

    Ok(())
}

#[macroquad::main("nertsio")]
async fn main() {
    let async_rt = tokio::runtime::Runtime::new().unwrap();

    let card_size = mq::Vec2::new(CARD_WIDTH, CARD_HEIGHT);

    let cards_texture =
        mq::Texture2D::from_file_with_format(nertsio_textures::CARDS, Some(mq::ImageFormat::Png));
    let backs_texture =
        mq::Texture2D::from_file_with_format(nertsio_textures::BACKS, Some(mq::ImageFormat::Png));
    let placeholder_texture = mq::Texture2D::from_file_with_format(
        nertsio_textures::PLACEHOLDER,
        Some(mq::ImageFormat::Png),
    );

    let draw_card = |card: ni_ty::Card, x: f32, y: f32| {
        mq::draw_texture_ex(
            cards_texture,
            x,
            y,
            mq::WHITE,
            mq::DrawTextureParams {
                source: Some(get_card_rect(card)),
                dest_size: Some(card_size),
                ..Default::default()
            },
        );
    };

    let draw_back = |x: f32, y: f32| {
        mq::draw_texture_ex(
            backs_texture,
            x,
            y,
            mq::WHITE,
            mq::DrawTextureParams {
                source: Some(mq::Rect {
                    x: 10.0,
                    y: 10.0,
                    w: 120.0,
                    h: 180.0,
                }),
                dest_size: Some(card_size),
                ..Default::default()
            },
        );
    };

    let draw_placeholder = |x: f32, y: f32| {
        mq::draw_texture_ex(
            placeholder_texture,
            x,
            y,
            mq::WHITE,
            mq::DrawTextureParams {
                source: Some(mq::Rect {
                    x: 10.0,
                    y: 10.0,
                    w: 120.0,
                    h: 180.0,
                }),
                dest_size: Some(card_size),
                ..Default::default()
            },
        );
    };

    let draw_vertical_stack_cards = |cards: &[ni_ty::CardInstance], x: f32, y: f32| {
        if cards.is_empty() {
            draw_placeholder(x, y);
        } else {
            for (i, card) in cards.iter().enumerate() {
                draw_card(card.card, x, y + (i as f32) * VERTICAL_STACK_SPACING);
            }
        }
    };

    let draw_horizontal_stack_cards = |cards: &[ni_ty::CardInstance], x: f32, y: f32| {
        if cards.is_empty() {
            mq::draw_texture_ex(
                placeholder_texture,
                x,
                y,
                mq::WHITE,
                mq::DrawTextureParams {
                    source: Some(mq::Rect {
                        x: 10.0,
                        y: 10.0,
                        w: 120.0,
                        h: 180.0,
                    }),
                    dest_size: Some(card_size),
                    ..Default::default()
                },
            );
        } else {
            for (i, card) in cards.iter().enumerate() {
                draw_card(card.card, x + (i as f32) * HORIZONTAL_STACK_SPACING, y);
            }
        }
    };

    let game_info_mutex = Arc::new(std::sync::Mutex::new(None));
    let (game_msg_send, game_msg_recv) = tokio::sync::mpsc::unbounded_channel();

    let mut game_msg_recv_opt = Some(game_msg_recv);

    let http_client = hyper::Client::new();

    let mut do_connection = |connection_type| {
        async_rt.spawn({
            let game_info_mutex = game_info_mutex.clone();
            let game_msg_recv = game_msg_recv_opt.take().unwrap();
            let http_client = http_client.clone();
            async move {
                if let Err(err) = handle_connection(
                    &http_client,
                    connection_type,
                    &game_info_mutex,
                    game_msg_recv,
                )
                .await
                {
                    eprintln!("Failed to handle connection: {:?}", err);
                }
            }
        });
    };

    let mut state = State::MainMenu;

    loop {
        mq::set_default_camera();

        state = match state {
            State::MainMenu => {
                mq::clear_background(BACKGROUND_COLOR);

                if mqui::root_ui().button(mq::Vec2::new(10.0, 10.0), "Create Public Game") {
                    do_connection(ConnectionType::CreateGame { public: true });
                    State::Connecting
                } else if mqui::root_ui().button(mq::Vec2::new(10.0, 40.0), "Create Private Game") {
                    do_connection(ConnectionType::CreateGame { public: false });
                    State::Connecting
                } else if mqui::root_ui().button(mq::Vec2::new(10.0, 70.0), "Join Game") {
                    State::JoinGameForm {
                        input: String::new(),
                    }
                } else {
                    State::MainMenu
                }
            }
            State::JoinGameForm { mut input } => {
                use mqui::hash;

                mq::clear_background(BACKGROUND_COLOR);

                mqui::root_ui().input_text(hash!(), "Room Code", &mut input);
                if mqui::root_ui().button(None, "Join") {
                    if let Ok((server_id, game_id)) = parse_full_game_id_str(&input) {
                        do_connection(ConnectionType::JoinPrivateGame { server_id, game_id });
                        State::Connecting
                    } else {
                        State::JoinGameForm { input }
                    }
                } else {
                    State::JoinGameForm { input }
                }
            }
            State::Connecting => {
                mq::clear_background(BACKGROUND_COLOR);

                if (*game_info_mutex.lock().unwrap()).is_some() {
                    State::GameNeutral
                } else {
                    State::Connecting
                }
            }
            State::GameNeutral => {
                mq::clear_background(BACKGROUND_COLOR);

                let mut lock = game_info_mutex.lock().unwrap();
                let shared = (*lock).as_mut().unwrap();

                match &shared.game.hand {
                    None => {
                        mqui::root_ui().label(
                            mq::Vec2::new(20.0, 10.0),
                            &format!(
                                "Room Code: {}",
                                to_full_game_id_str(shared.server_id, shared.game.id),
                            ),
                        );

                        for (i, (key, player)) in shared.game.players.iter_mut().enumerate() {
                            let y = 30.0 + (i as f32) * 25.0;

                            mqui::root_ui()
                                .label(mq::Vec2::new(10.0, y), &player.score.to_string());

                            if *key == shared.my_player_id {
                                if mqui::root_ui().button(
                                    mq::Vec2::new(30.0, y),
                                    if player.ready { "Unready" } else { "Ready" },
                                ) {
                                    let new_value = !player.ready;
                                    player.ready = new_value;

                                    game_msg_send
                                        .send(ni_ty::protocol::GameMessageC2S::UpdateSelfReady {
                                            value: new_value,
                                        })
                                        .unwrap();
                                }
                            } else {
                                mqui::root_ui().label(
                                    mq::Vec2::new(30.0, y),
                                    if player.ready { "Ready" } else { "Not Ready" },
                                );
                            }
                        }

                        State::GameNeutral
                    }
                    Some(hand) => State::GameHand {
                        my_player_idx: hand
                            .players()
                            .iter()
                            .position(|player| player.player_id() == shared.my_player_id)
                            .unwrap(),
                    },
                }
            }
            State::GameHand { my_player_idx } => {
                let my_player_idx_u8 = my_player_idx as u8;

                let mut lock = game_info_mutex.lock().unwrap();
                let shared = (*lock).as_mut().unwrap();

                if let Some(real_hand_state) = shared.game.hand.as_mut() {
                    let player_hand_width = 130.0
                        + CARD_WIDTH
                        + (real_hand_state.players()[0].tableau_stacks().len() as f32)
                            * (CARD_WIDTH + 10.0);

                    let lake_width = ((real_hand_state.lake_stacks().len() as f32) * CARD_WIDTH)
                        + ((real_hand_state.lake_stacks().len() - 1) as f32) * LAKE_SPACING;

                    let player_count = real_hand_state.players().len();
                    let min_side_player_count = player_count / 2;

                    let max_side_player_count = if player_count % 2 == 0 {
                        min_side_player_count
                    } else {
                        min_side_player_count + 1
                    };

                    let max_side_width = player_hand_width * (max_side_player_count as f32)
                        + PLAYER_SPACING * ((max_side_player_count - 1) as f32);

                    let needed_screen_width = lake_width.max(max_side_width);
                    let needed_screen_height = PLAYER_Y * 2.0 + CARD_HEIGHT + 10.0 + CARD_HEIGHT;

                    let real_screen_size = (mq::screen_width(), mq::screen_height());
                    let screen_size = if real_screen_size.0 > needed_screen_width * 2.0
                        && real_screen_size.1 > needed_screen_height * 2.0
                    {
                        (real_screen_size.0 / 2.0, real_screen_size.1 / 2.0)
                    } else if real_screen_size.0 < needed_screen_width
                        || real_screen_size.1 < needed_screen_height
                    {
                        let factor = (needed_screen_width / real_screen_size.0)
                            .max(needed_screen_height / real_screen_size.1);
                        (real_screen_size.0 * factor, real_screen_size.1 * factor)
                    } else {
                        real_screen_size
                    };

                    let normal_camera = mq::Camera2D::from_display_rect(
                        mq::Rect::new(0.0, 0.0, screen_size.0, screen_size.1).into(),
                    );

                    let inverted_camera: mq::Camera2D = {
                        let mut res = normal_camera.clone();
                        res.rotation = 180.0;
                        res
                    };

                    let screen_center = (screen_size.0 / 2.0, screen_size.1 / 2.0);

                    let lake_start_x = screen_center.0 - lake_width / 2.0;

                    let get_player_location = |player_idx: usize| {
                        let inverted = player_idx >= min_side_player_count;
                        let side_player_count = if inverted && (player_count % 2 != 0) {
                            min_side_player_count + 1
                        } else {
                            min_side_player_count
                        };
                        let player_side_idx = if inverted {
                            player_idx - min_side_player_count
                        } else {
                            player_idx
                        };

                        let side_width = (player_hand_width * (side_player_count as f32))
                            + PLAYER_SPACING * (side_player_count - 1) as f32;

                        let x = -(side_width / 2.0)
                            + (player_hand_width + PLAYER_SPACING) * (player_side_idx as f32);

                        (x, inverted)
                    };

                    let my_location = get_player_location(my_player_idx);

                    let mut pred_hand_state = (*real_hand_state).clone();
                    for action in shared.pending_actions.iter() {
                        let _ = pred_hand_state.apply(my_player_idx_u8, *action);
                        // ignore error, will get cleared out eventually
                    }
                    if shared.self_called_nerts {
                        pred_hand_state.nerts_called = true;
                    }

                    let mouse_pos = mq::mouse_position();
                    let mouse_pos = mq::Vec2::new(
                        mouse_pos.0 * screen_size.0 / real_screen_size.0,
                        mouse_pos.1 * screen_size.1 / real_screen_size.1,
                    );

                    {
                        let player_state = &pred_hand_state.players()[my_player_idx];
                        let my_position =
                            (screen_center.0 + my_location.0, screen_center.1 + PLAYER_Y);

                        let mouse_pressed = mq::is_mouse_button_pressed(mq::MouseButton::Left);

                        if mouse_pressed || mq::is_mouse_button_released(mq::MouseButton::Left) {
                            if mq::Rect::new(
                                my_position.0,
                                my_position.1 + (CARD_HEIGHT + 10.0),
                                CARD_WIDTH,
                                CARD_HEIGHT,
                            )
                            .contains(mouse_pos)
                            {
                                if mouse_pressed {
                                    let action = if player_state.stock_stack().len() > 0 {
                                        ni_ty::HandAction::FlipStock
                                    } else {
                                        ni_ty::HandAction::ReturnStock
                                    };

                                    if pred_hand_state.apply(my_player_idx_u8, action).is_ok() {
                                        shared.pending_actions.push_back(action);
                                        game_msg_send
                                            .send(
                                                ni_ty::protocol::GameMessageC2S::ApplyHandAction {
                                                    action,
                                                },
                                            )
                                            .unwrap();

                                        shared.my_held_state = None;
                                    }
                                }
                            } else {
                                let found = if player_state.nerts_stack().len() > 0
                                    && mq::Rect::new(
                                        my_position.0
                                            + ((player_state.nerts_stack().len() - 1) as f32)
                                                * 10.0,
                                        my_position.1,
                                        CARD_WIDTH,
                                        CARD_HEIGHT,
                                    )
                                    .contains(mouse_pos)
                                {
                                    Some((
                                        ni_ty::StackLocation::Player(
                                            my_player_idx_u8,
                                            ni_ty::PlayerStackLocation::Nerts,
                                        ),
                                        1,
                                        mouse_pos
                                            - mq::Vec2::new(
                                                my_position.0
                                                    + ((player_state.nerts_stack().len() - 1)
                                                        as f32)
                                                        * 10.0,
                                                my_position.1,
                                            ),
                                    ))
                                } else if mq::Rect::new(
                                    lake_start_x,
                                    screen_center.1 - CARD_HEIGHT / 2.0,
                                    lake_width,
                                    CARD_HEIGHT,
                                )
                                .contains(mouse_pos)
                                {
                                    let stack_idx_for_me = ((mouse_pos[0] - lake_start_x)
                                        / (CARD_WIDTH + LAKE_SPACING))
                                        as u16;

                                    let stack_idx = if my_location.1 {
                                        (pred_hand_state.lake_stacks().len() as u16)
                                            - stack_idx_for_me
                                            - 1
                                    } else {
                                        stack_idx_for_me
                                    };

                                    Some((
                                        ni_ty::StackLocation::Lake(stack_idx),
                                        1,
                                        mouse_pos
                                            - mq::Vec2::new(
                                                lake_start_x
                                                    + (CARD_WIDTH + LAKE_SPACING)
                                                        * (stack_idx as f32),
                                                screen_center.1 - CARD_HEIGHT / 2.0,
                                            ),
                                    ))
                                } else if mq::Rect::new(
                                    my_position.0 + CARD_WIDTH + 10.0,
                                    my_position.1 + (CARD_HEIGHT + 10.0),
                                    CARD_WIDTH + HORIZONTAL_STACK_SPACING * 2.0,
                                    CARD_HEIGHT,
                                )
                                .contains(mouse_pos)
                                {
                                    Some((
                                        ni_ty::StackLocation::Player(
                                            my_player_idx_u8,
                                            ni_ty::PlayerStackLocation::Waste,
                                        ),
                                        1,
                                        mouse_pos
                                            - mq::Vec2::new(
                                                my_position.0
                                                    + CARD_WIDTH
                                                    + 10.0
                                                    + (HORIZONTAL_STACK_SPACING
                                                        * ((player_state.waste_stack().len().min(3)
                                                            - 1)
                                                            as f32)),
                                                my_position.1 + CARD_HEIGHT + 10.0,
                                            ),
                                    ))
                                } else {
                                    player_state
                                        .tableau_stacks()
                                        .iter()
                                        .enumerate()
                                        .filter_map(|(i, stack)| {
                                            let x = my_position.0
                                                + 130.0
                                                + CARD_WIDTH
                                                + (i as f32) * (CARD_WIDTH + 10.0);
                                            if mq::Rect::new(
                                                x,
                                                my_position.1,
                                                CARD_WIDTH,
                                                CARD_HEIGHT
                                                    + ((stack.len() as f32) - 1.0)
                                                        * VERTICAL_STACK_SPACING,
                                            )
                                            .contains(mouse_pos)
                                            {
                                                let loc = ni_ty::StackLocation::Player(
                                                    my_player_idx_u8,
                                                    ni_ty::PlayerStackLocation::Tableau(i as u8),
                                                );
                                                if stack.len() > 0 {
                                                    let found_idx = (((mouse_pos[1]
                                                        - my_position.1)
                                                        / VERTICAL_STACK_SPACING)
                                                        as usize)
                                                        .min(stack.len() - 1);

                                                    Some((
                                                        loc,
                                                        stack.len() - found_idx,
                                                        mouse_pos
                                                            - mq::Vec2::new(
                                                                x,
                                                                my_position.1
                                                                    + ((found_idx as f32)
                                                                        * VERTICAL_STACK_SPACING),
                                                            ),
                                                    ))
                                                } else {
                                                    Some((
                                                        loc,
                                                        0,
                                                        mouse_pos - mq::Vec2::new(x, my_position.1),
                                                    ))
                                                }
                                            } else {
                                                None
                                            }
                                        })
                                        .next()
                                };

                                let _ = player_state;

                                println!("click found {:?}", found);

                                if let Some(found) = found {
                                    match shared.my_held_state {
                                        None => {
                                            if mouse_pressed {
                                                if let (
                                                    ni_ty::StackLocation::Player(_, src),
                                                    count,
                                                    offset,
                                                ) = found
                                                {
                                                    let stack =
                                                        pred_hand_state.stack_at(found.0).unwrap();
                                                    if stack.len() > 0 {
                                                        let top_card = stack.cards()
                                                            [stack.cards().len() - count]
                                                            .card;

                                                        shared.my_held_state =
                                                            Some(ni_ty::HeldInfo {
                                                                src,
                                                                count: count as u8,
                                                                offset: (offset[0], offset[1]),
                                                                top_card,
                                                            });
                                                    }
                                                }
                                            }
                                        }
                                        Some(ref held) => {
                                            let src_loc = ni_ty::StackLocation::Player(
                                                my_player_idx_u8,
                                                held.src,
                                            );

                                            let (target_loc, ..) = found;
                                            if target_loc == src_loc {
                                                if mouse_pressed {
                                                    shared.my_held_state = None;
                                                }
                                            } else {
                                                if matches!(
                                                    target_loc,
                                                    ni_ty::StackLocation::Player(
                                                        _,
                                                        ni_ty::PlayerStackLocation::Tableau(_)
                                                    ) | ni_ty::StackLocation::Lake(_)
                                                ) {
                                                    if let Some(target_stack) =
                                                        pred_hand_state.stack_at(target_loc)
                                                    {
                                                        if let Some(src_stack) =
                                                            pred_hand_state.stack_at(src_loc)
                                                        {
                                                            let stack_cards = src_stack.cards();
                                                            let back_card = &stack_cards[stack_cards
                                                                .len()
                                                                - held.count as usize];

                                                            if target_stack.can_add(*back_card) {
                                                                let action =
                                                                    ni_ty::HandAction::Move {
                                                                        from: src_loc,
                                                                        count: held.count,
                                                                        to: target_loc,
                                                                    };

                                                                println!("applying for check");
                                                                if pred_hand_state
                                                                    .apply(my_player_idx_u8, action)
                                                                    .is_ok()
                                                                {
                                                                    shared
                                                                        .pending_actions
                                                                        .push_back(action);
                                                                    game_msg_send.send(ni_ty::protocol::GameMessageC2S::ApplyHandAction { action }).unwrap();
                                                                }

                                                                shared.my_held_state = None;
                                                            } else {
                                                                println!(
                                                                    "can't add {:?} to {:?}",
                                                                    back_card, target_stack
                                                                );
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

                    mq::clear_background(BACKGROUND_COLOR);

                    for (idx, player_state) in pred_hand_state.players().iter().enumerate() {
                        let location = get_player_location(idx);
                        let position = (screen_center.0 + location.0, screen_center.1 + PLAYER_Y);

                        if location.1 != my_location.1 {
                            mq::set_camera(&inverted_camera);
                        } else {
                            mq::set_camera(&normal_camera);
                        }

                        let held_state = if idx == my_player_idx {
                            shared.my_held_state
                        } else {
                            shared.hand_mouse_states.as_ref().unwrap()[idx]
                                .as_ref()
                                .and_then(|(_, state)| state.held)
                                .and_then(|held| {
                                    let stack = player_state.stack_at(held.src);
                                    if let Some(stack) = stack {
                                        let cards = stack.cards();
                                        if (held.count as usize) <= cards.len() {
                                            let cards =
                                                &cards[(cards.len() - held.count as usize)..];

                                            if cards[0].card == held.top_card {
                                                Some(held)
                                            } else {
                                                None
                                            }
                                        } else {
                                            None
                                        }
                                    } else {
                                        None
                                    }
                                })
                        };

                        if player_state.nerts_stack().len() > 0 {
                            for i in 0..(player_state.nerts_stack().len() - 1) {
                                draw_back(position.0 + (i as f32) * 10.0, position.1);
                            }
                            let card = player_state.nerts_stack().last().unwrap();

                            if !matches!(
                                held_state,
                                Some(ni_ty::HeldInfo {
                                    src: ni_ty::PlayerStackLocation::Nerts,
                                    ..
                                })
                            ) {
                                draw_card(
                                    card.card,
                                    position.0
                                        + ((player_state.nerts_stack().len() - 1) as f32) * 10.0,
                                    position.1,
                                );
                            }
                        } else {
                            if idx == my_player_idx {
                                if mqui::root_ui().button(
                                    mq::Vec2::new(
                                        position.0 + CARD_WIDTH / 2.0,
                                        position.1 + CARD_HEIGHT / 2.0,
                                    ),
                                    "Nerts!",
                                ) {
                                    shared.self_called_nerts = true;
                                    game_msg_send
                                        .send(ni_ty::protocol::GameMessageC2S::CallNerts)
                                        .unwrap();
                                }
                            }
                        }

                        for (i, stack) in player_state.tableau_stacks().iter().enumerate() {
                            let cards = stack.cards();
                            let cards = if let Some(ni_ty::HeldInfo {
                                src: ni_ty::PlayerStackLocation::Tableau(stack_idx),
                                count,
                                ..
                            }) = held_state
                            {
                                if i == (stack_idx as usize) {
                                    &cards[..(cards.len() - count as usize)]
                                } else {
                                    cards
                                }
                            } else {
                                cards
                            };

                            draw_vertical_stack_cards(
                                cards,
                                position.0 + 130.0 + CARD_WIDTH + (i as f32) * (CARD_WIDTH + 10.0),
                                position.1,
                            );
                        }

                        let stock_pos = (position.0, position.1 + CARD_HEIGHT + 10.0);
                        if player_state.stock_stack().len() > 0 {
                            draw_back(stock_pos.0, stock_pos.1);
                        } else {
                            draw_placeholder(stock_pos.0, stock_pos.1);
                        }

                        let waste_cards = player_state.waste_stack().cards();
                        let waste_cards = if waste_cards.len() > 3 {
                            &waste_cards[(waste_cards.len() - 3)..]
                        } else {
                            waste_cards
                        };
                        let waste_cards = if let Some(ni_ty::HeldInfo {
                            src: ni_ty::PlayerStackLocation::Waste,
                            count,
                            ..
                        }) = held_state
                        {
                            &waste_cards[..(waste_cards.len() - count as usize)]
                        } else {
                            waste_cards
                        };

                        if waste_cards.len() > 0 {
                            draw_horizontal_stack_cards(
                                waste_cards,
                                stock_pos.0 + CARD_WIDTH + 10.0,
                                stock_pos.1,
                            );
                        }
                    }

                    if my_location.1 {
                        mq::set_camera(&inverted_camera);
                    } else {
                        mq::set_camera(&normal_camera);
                    }

                    for (i, stack) in pred_hand_state.lake_stacks().iter().enumerate() {
                        let x = lake_start_x + (i as f32) * (CARD_WIDTH + LAKE_SPACING);
                        let y = screen_center.1 - CARD_HEIGHT / 2.0;

                        match stack.cards().last() {
                            None => {
                                draw_placeholder(x, y);
                            }
                            Some(card) => {
                                draw_card(card.card, x, y);
                            }
                        }
                    }

                    for (idx, value) in shared
                        .hand_mouse_states
                        .as_ref()
                        .unwrap()
                        .iter()
                        .enumerate()
                    {
                        if let Some((_, state)) = value {
                            let location = get_player_location(idx);

                            if location.1 != my_location.1 {
                                mq::set_camera(&inverted_camera);
                            } else {
                                mq::set_camera(&normal_camera);
                            }

                            if let Some(held) = state.held {
                                println!("player held {:?}", held);

                                let stack = pred_hand_state.players()[idx].stack_at(held.src);
                                if let Some(stack) = stack {
                                    let cards = stack.cards();
                                    if (held.count as usize) <= cards.len() {
                                        let cards = &cards[(cards.len() - held.count as usize)..];

                                        if cards[0].card == held.top_card {
                                            draw_vertical_stack_cards(
                                                cards,
                                                screen_center.0 + state.position.0 - held.offset.0,
                                                screen_center.1 + state.position.1 - held.offset.1,
                                            );
                                        }
                                    }
                                }
                            }

                            mq::draw_rectangle(
                                screen_center.0 + state.position.0 - OTHER_CURSOR_SIZE / 2.0,
                                screen_center.1 + state.position.1 - OTHER_CURSOR_SIZE / 2.0,
                                OTHER_CURSOR_SIZE,
                                OTHER_CURSOR_SIZE,
                                mq::BLACK,
                            );
                        }
                    }

                    mq::set_camera(&normal_camera);

                    let my_player_state = &pred_hand_state.players()[my_player_idx];
                    if let Some(ref held) = shared.my_held_state {
                        let stack = my_player_state.stack_at(held.src);
                        if let Some(stack) = stack {
                            let stack_cards = stack.cards();
                            let cards = &stack_cards[(stack_cards.len() - held.count as usize)..];

                            draw_vertical_stack_cards(
                                cards,
                                mouse_pos[0] - held.offset.0,
                                mouse_pos[1] - held.offset.1,
                            );
                        } else {
                            shared.my_held_state = None;
                        }
                    }

                    if pred_hand_state.nerts_called {
                        mq::draw_text("Nerts!", screen_center.0, screen_center.1, 100.0, mq::BLACK);
                    }

                    shared.last_mouse_position = Some((
                        mouse_pos[0] - screen_center.0,
                        mouse_pos[1] - screen_center.1,
                    ));

                    State::GameHand { my_player_idx }
                } else {
                    State::GameNeutral
                }
            }
        };

        mq::next_frame().await
    }
}
