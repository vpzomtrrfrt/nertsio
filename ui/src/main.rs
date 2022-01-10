use futures_util::FutureExt;
use macroquad::prelude as mq;
use macroquad::ui as mqui;
use nertsio_types as ni_ty;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncSeekExt, AsyncWriteExt};

mod connection;

use connection::ConnectionMessage;

const BACKGROUND_COLOR: mq::Color = mq::Color::new(0.2, 0.7, 0.2, 1.0);
const NERTS_OVERLAY_COLOR: mq::Color = mq::Color::new(1.0, 1.0, 1.0, 0.4);
const NERTS_TEXT_COLOR: mq::Color = mq::Color::new(0.0, 0.0, 1.0, 1.0);

const PLAYER_COLORS: [mq::Color; 16] = [
    mq::Color::new(1.0, 0.0, 0.0, 1.0),
    mq::Color::new(1.0, 0.3, 0.0, 1.0),
    mq::Color::new(1.0, 0.7, 0.0, 1.0),
    mq::Color::new(0.9, 1.0, 0.0, 1.0),
    mq::Color::new(0.6, 1.0, 0.0, 1.0),
    mq::Color::new(0.2, 1.0, 0.0, 1.0),
    mq::Color::new(0.0, 1.0, 0.1, 1.0),
    mq::Color::new(0.0, 1.0, 0.5, 1.0),
    mq::Color::new(0.0, 1.0, 0.8, 1.0),
    mq::Color::new(0.0, 0.8, 1.0, 1.0),
    mq::Color::new(0.0, 0.5, 1.0, 1.0),
    mq::Color::new(0.0, 0.1, 1.0, 1.0),
    mq::Color::new(0.2, 0.0, 1.0, 1.0),
    mq::Color::new(0.6, 0.0, 1.0, 1.0),
    mq::Color::new(0.9, 0.0, 1.0, 1.0),
    mq::Color::new(1.0, 0.0, 0.7, 1.0),
];

const CARD_WIDTH: f32 = 90.0;
const CARD_HEIGHT: f32 = 135.0;
const LAKE_SPACING: f32 = 10.0;
const HORIZONTAL_STACK_SPACING: f32 = 10.0;
const VERTICAL_STACK_SPACING: f32 = 20.0;
const PLAYER_SPACING: f32 = 20.0;
const PLAYER_Y: f32 = 200.0;

const GAME_ID_FORMAT: u128 = lexical::NumberFormatBuilder::from_radix(36);

const COORDINATOR_URL: &str = "http://coordinator.nerts.io/";
// const COORDINATOR_URL: &str = "http://localhost:6462/";

fn default_name() -> String {
    "Nerter".to_owned()
}

#[derive(Clone, PartialEq, Deserialize, Serialize)]
struct Settings {
    #[serde(default = "default_name")]
    name: String,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            name: default_name(),
        }
    }
}

enum ConnectionState {
    NotConnected { expected: bool },
    Connecting,
    Connected(SharedInfo),
}

impl ConnectionState {
    pub fn as_info_mut(&mut self) -> Option<&mut SharedInfo> {
        match self {
            ConnectionState::NotConnected { expected: _ } | ConnectionState::Connecting => None,
            ConnectionState::Connected(info) => Some(info),
        }
    }
}

struct SharedInfo {
    game: ni_ty::GameState,
    my_player_id: u8,
    server_id: u8,
    hand_extra: Option<HandExtra>,
    new_end_scores: Option<Vec<(u8, i32)>>,
}

struct HandExtra {
    pending_actions: VecDeque<ni_ty::HandAction>,
    self_called_nerts: bool,
    mouse_states: Vec<Option<(u32, ni_ty::MouseState)>>,
    my_held_state: Option<ni_ty::HeldInfo>,
    last_mouse_position: Option<(f32, f32)>,
    stalled: bool,
}

impl HandExtra {
    pub fn new(player_count: usize) -> Self {
        Self {
            pending_actions: Default::default(),
            self_called_nerts: false,
            mouse_states: vec![None; player_count],
            my_held_state: None,
            last_mouse_position: None,
            stalled: false,
        }
    }
}

enum State {
    MainMenu,
    JoinGameForm {
        input: String,
    },
    PublicGameListLoading {
        channel: tokio::sync::oneshot::Receiver<Vec<ni_ty::protocol::PublicGameInfoExpanded>>,
    },
    PublicGameList {
        list: Vec<ni_ty::protocol::PublicGameInfoExpanded>,
    },
    Connecting,
    GameNeutral,
    GameHand {
        my_player_idx: Option<usize>,
    },
    GameEnd {
        scores: Vec<(u8, i32)>,
    },
    LostConnection {
        was_connected: bool,
    },
}

fn parse_full_game_id_str(src: &str) -> Result<(u8, u32), lexical::Error> {
    let result: u64 = lexical::parse_with_options::<_, _, GAME_ID_FORMAT>(
        &src,
        &lexical::parse_integer_options::Options::default(),
    )?;

    Ok(((result & (u8::MAX as u64)) as u8, (result >> 8) as u32))
}

fn to_full_game_id_str(server_id: u8, game_id: u32) -> String {
    lexical::to_string_with_options::<_, GAME_ID_FORMAT>(
        (u64::from(game_id) << 8) + u64::from(server_id),
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

async fn res_to_error(
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

async fn run_settings_save_loop(
    file: std::fs::File,
    init_value: Settings,
    mutex: Arc<Mutex<Settings>>,
) {
    let mut saved_value = init_value;

    let mut file = tokio::fs::File::from_std(file);

    let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        interval.tick().await;

        if {
            let lock = mutex.lock().unwrap();
            if saved_value != *lock {
                saved_value = (*lock).clone();
                true
            } else {
                false
            }
        } {
            // value changed, need to save it

            if let Err(err) = async {
                let buf = serde_json::to_vec(&saved_value)?;

                file.rewind().await?;
                file.write_all(&buf).await?;

                Result::<_, anyhow::Error>::Ok(())
            }
            .await
            {
                eprintln!("failed to save settings: {:?}", err);
            }
        }
    }
}

fn get_window_conf() -> mq::Conf {
    mq::Conf {
        window_title: "nertsio".to_owned(),
        icon: Some(macroquad::miniquad::conf::Icon {
            small: nertsio_textures::ICON_PIXELS_16.try_into().unwrap(),
            medium: nertsio_textures::ICON_PIXELS_32.try_into().unwrap(),
            big: nertsio_textures::ICON_PIXELS_64.try_into().unwrap(),
        }),
        ..Default::default()
    }
}

#[macroquad::main(get_window_conf)]
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
    let cursors_texture =
        mq::Texture2D::from_file_with_format(nertsio_textures::CURSORS, Some(mq::ImageFormat::Png));

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

    let draw_back = |x: f32, y: f32, owner_id: u8| {
        let bg_color = PLAYER_COLORS[(owner_id >> 4) as usize];
        let fg_color = PLAYER_COLORS[(owner_id & 0xF) as usize];

        mq::draw_texture_ex(
            backs_texture,
            x,
            y,
            bg_color,
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

        mq::draw_texture_ex(
            backs_texture,
            x,
            y,
            fg_color,
            mq::DrawTextureParams {
                source: Some(mq::Rect {
                    x: 140.0,
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

    let game_info_mutex = Arc::new(std::sync::Mutex::new(ConnectionState::NotConnected {
        expected: true,
    }));
    let game_msg_send = RefCell::new(None);

    let http_client = hyper::Client::new();

    let settings_mutex;
    {
        let config_dir = dirs::config_dir()
            .map(Cow::Owned)
            .unwrap_or_else(|| std::path::Path::new(".").into());
        match std::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(config_dir.join("nertsio.json"))
        {
            Ok(mut file) => {
                let init_value: Settings = match serde_json::from_reader(&mut file) {
                    Ok(value) => value,
                    Err(err) => {
                        println!("Failed to parse config file: {:?}", err);
                        println!("Will reset config to defaults.");

                        Default::default()
                    }
                };

                settings_mutex = Arc::new(Mutex::new(init_value.clone()));
                async_rt.spawn(run_settings_save_loop(
                    file,
                    init_value,
                    settings_mutex.clone(),
                ));
            }
            Err(err) => {
                eprintln!("Failed to open settings file: {:?}", err);
                eprintln!("Settings will not be saved.");

                settings_mutex = Arc::new(Mutex::new(Default::default()));
            }
        }
    }

    let do_connection = |connection_type| {
        let (new_game_msg_send, game_msg_recv) = tokio::sync::mpsc::unbounded_channel();
        *game_msg_send.borrow_mut() = Some(new_game_msg_send);

        async_rt.spawn({
            let game_info_mutex = game_info_mutex.clone();
            let http_client = http_client.clone();
            let settings_mutex = settings_mutex.clone();
            async move {
                {
                    (*game_info_mutex.lock().unwrap()) = ConnectionState::Connecting;
                }
                let res = connection::handle_connection(
                    &http_client,
                    connection_type,
                    &game_info_mutex,
                    game_msg_recv,
                    settings_mutex,
                )
                .await;

                let mut lock = game_info_mutex.lock().unwrap();
                if let Err(err) = res {
                    (*lock) = ConnectionState::NotConnected { expected: false };
                    eprintln!("Failed to handle connection: {:?}", err);
                } else {
                    (*lock) = ConnectionState::NotConnected { expected: true };
                }
            }
        });
    };

    let start_loading_public_games = || {
        let (send, recv) = tokio::sync::oneshot::channel();

        let req_fut = http_client.request(
            hyper::Request::get(format!("{}public_games", COORDINATOR_URL))
                .body(Default::default())
                .unwrap(),
        );
        async_rt.spawn(
            (async move {
                let resp = res_to_error(req_fut.await?).await?;

                let resp = hyper::body::to_bytes(resp.into_body()).await?;
                let resp: ni_ty::protocol::RespList<ni_ty::protocol::PublicGameInfoExpanded> =
                    serde_json::from_slice(&resp)?;

                let _ = send.send(resp.items); // if this fails, then we didn't need it anyway

                Result::<_, anyhow::Error>::Ok(())
            })
            .then(|res| {
                if let Err(err) = res {
                    eprintln!("Failed to list public games: {:?}", err);
                }

                futures_util::future::ready(())
            }),
        );

        recv
    };

    let draw_text_centered = |text: &str, x, y, font_size, color| {
        let metrics = mq::measure_text(
            text,
            None,
            font_size,
            mq::camera_font_scale(font_size.into()).1,
        );

        mq::draw_text(
            text,
            x - metrics.width / 2.0,
            y - metrics.height / 2.0 + metrics.offset_y,
            font_size.into(),
            color,
        );
    };

    let skin = {
        let style = mqui::root_ui()
            .style_builder()
            .font_size(64)
            .color_hovered(mq::Color::from_rgba(170, 170, 170, 255))
            .build();

        let mut skin = mqui::root_ui().default_skin().clone();
        skin.label_style = style.clone();
        skin.button_style = style.clone();
        skin.editbox_style = style;

        skin
    };

    mqui::root_ui().push_skin(&skin);

    let mut state = State::MainMenu;

    loop {
        mq::set_default_camera();

        state = match state {
            State::MainMenu => {
                mq::clear_background(BACKGROUND_COLOR);

                let button_height = 50.0;
                let button_spacing = 25.0;

                let button_count = 5;

                let menu_width = 600.0;
                let menu_height = button_height * (button_count as f32)
                    + ((button_count - 1) as f32) * button_spacing;
                let menu_x = mq::screen_width() / 2.0 - menu_width / 2.0;
                let menu_y = mq::screen_height() / 2.0 - menu_height / 2.0;

                let menu_button = |idx, label| {
                    mqui::widgets::Button::new(label)
                        .position(mq::Vec2::new(
                            menu_x,
                            menu_y + (button_height + button_spacing) * (idx as f32),
                        ))
                        .size(mq::Vec2::new(menu_width, button_height))
                        .ui(&mut mqui::root_ui())
                };

                {
                    use mqui::hash;

                    let mut settings_lock = settings_mutex.lock().unwrap();
                    let settings = &mut *settings_lock;

                    mqui::widgets::InputText::new(hash!())
                        .label("Name")
                        .size(mq::Vec2::new(menu_width, button_height + 20.0))
                        .position(mq::Vec2::new(menu_x, menu_y - 20.0))
                        .ratio(0.8)
                        .ui(&mut mqui::root_ui(), &mut settings.name);
                }

                if menu_button(1, "Create Public Game") {
                    do_connection(connection::ConnectionType::CreateGame { public: true });
                    State::Connecting
                } else if menu_button(2, "Create Private Game") {
                    do_connection(connection::ConnectionType::CreateGame { public: false });
                    State::Connecting
                } else if menu_button(3, "Join Public Game") {
                    let channel = start_loading_public_games();
                    State::PublicGameListLoading { channel }
                } else if menu_button(4, "Join Private Game") {
                    State::JoinGameForm {
                        input: String::new(),
                    }
                } else {
                    State::MainMenu
                }
            }
            State::JoinGameForm { mut input } => {
                use mqui::hash;

                let screen_center = (mq::screen_width() / 2.0, mq::screen_height() / 2.0);

                mq::clear_background(BACKGROUND_COLOR);

                mqui::widgets::InputText::new(hash!())
                    .label("Room Code")
                    .size(mq::Vec2::new(500.0, 70.0))
                    .position(mq::Vec2::new(
                        screen_center.0 - 250.0,
                        screen_center.1 - 35.0,
                    ))
                    .ui(&mut mqui::root_ui(), &mut input);
                if mqui::widgets::Button::new("Join")
                    .position(mq::Vec2::new(
                        screen_center.0 - 100.0,
                        screen_center.1 + 70.0,
                    ))
                    .size(mq::Vec2::new(200.0, 50.0))
                    .ui(&mut mqui::root_ui())
                {
                    if let Ok((server_id, game_id)) = parse_full_game_id_str(&input) {
                        do_connection(connection::ConnectionType::JoinPrivateGame {
                            server_id,
                            game_id,
                        });
                        State::Connecting
                    } else {
                        State::JoinGameForm { input }
                    }
                } else {
                    if mqui::root_ui().button(mq::Vec2::new(10.0, 10.0), "Back") {
                        State::MainMenu
                    } else {
                        if mq::is_key_pressed(mq::KeyCode::Escape) {
                            State::MainMenu
                        } else {
                            State::JoinGameForm { input }
                        }
                    }
                }
            }
            State::PublicGameListLoading { mut channel } => {
                mq::clear_background(BACKGROUND_COLOR);

                let screen_center = (mq::screen_width() / 2.0, mq::screen_height() / 2.0);

                draw_text_centered(
                    "Loading...",
                    screen_center.0,
                    screen_center.1,
                    60,
                    mq::BLACK,
                );

                if mq::is_key_pressed(mq::KeyCode::Escape) {
                    State::MainMenu
                } else {
                    match channel.try_recv() {
                        Ok(list) => State::PublicGameList { list },
                        Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {
                            State::PublicGameListLoading { channel }
                        }
                        Err(tokio::sync::oneshot::error::TryRecvError::Closed) => State::MainMenu,
                    }
                }
            }
            State::PublicGameList { list } => {
                mq::clear_background(BACKGROUND_COLOR);

                let screen_center = (mq::screen_width() / 2.0, mq::screen_height() / 2.0);

                let row_height = 50.0;
                let spacing = 25.0;
                let row_width = 1000.0;

                let list_height =
                    (row_height * list.len() as f32) + (spacing * (list.len() - 1) as f32);
                let list_x = screen_center.0 - row_width / 2.0;
                let list_y = screen_center.1 - list_height / 2.0;

                let mut joining = None;
                for (idx, game) in list.iter().enumerate() {
                    let y = list_y + (idx as f32) * (row_height + spacing);

                    mqui::root_ui().label(
                        mq::Vec2::new(list_x, y),
                        &to_full_game_id_str(game.server.server_id, game.game_id),
                    );
                    mqui::root_ui().label(
                        mq::Vec2::new(list_x + 270.0, y),
                        &format!("{} players", game.players),
                    );
                    mqui::root_ui().label(
                        mq::Vec2::new(list_x + 570.0, y),
                        if game.waiting { "waiting" } else { "playing" },
                    );
                    if mqui::root_ui().button(mq::Vec2::new(list_x + 820.0, y), "Join") {
                        joining = Some(game);
                    }
                }

                match joining {
                    None => {
                        if mqui::root_ui().button(mq::Vec2::new(10.0, 10.0), "Back") {
                            State::MainMenu
                        } else {
                            if mq::is_key_pressed(mq::KeyCode::Escape) {
                                State::MainMenu
                            } else {
                                State::PublicGameList { list }
                            }
                        }
                    }
                    Some(game) => {
                        do_connection(connection::ConnectionType::JoinPublicGame {
                            server: game.server.clone(),
                            game_id: game.game_id,
                        });
                        State::Connecting
                    }
                }
            }
            State::Connecting => {
                mq::clear_background(BACKGROUND_COLOR);

                if mq::is_key_pressed(mq::KeyCode::Escape) {
                    game_msg_send
                        .borrow()
                        .as_ref()
                        .unwrap()
                        .send(ConnectionMessage::Leave)
                        .unwrap();
                }

                match *game_info_mutex.lock().unwrap() {
                    ConnectionState::Connecting => State::Connecting,
                    ConnectionState::Connected(_) => State::GameNeutral,
                    ConnectionState::NotConnected { expected } => {
                        if expected {
                            State::MainMenu
                        } else {
                            State::LostConnection {
                                was_connected: false,
                            }
                        }
                    }
                }
            }
            State::GameNeutral => {
                mq::clear_background(BACKGROUND_COLOR);

                let mut lock = game_info_mutex.lock().unwrap();
                if let Some(shared) = (*lock).as_info_mut() {
                    match &shared.game.hand {
                        None => {
                            if let Some(scores) = shared.new_end_scores.take() {
                                State::GameEnd { scores }
                            } else {
                                mqui::root_ui().label(
                                    mq::Vec2::new(60.0, 100.0),
                                    &format!(
                                        "Room Code: {}",
                                        to_full_game_id_str(shared.server_id, shared.game.id),
                                    ),
                                );

                                let sorted = {
                                    let mut result: Vec<u8> =
                                        shared.game.players.keys().copied().collect();
                                    result.sort_by_key(|key| -shared.game.players[key].score);
                                    result
                                };

                                for (i, key) in sorted.iter().enumerate() {
                                    let player = shared.game.players.get_mut(key).unwrap();

                                    let y = 160.0 + (i as f32) * 75.0;

                                    mqui::root_ui()
                                        .label(mq::Vec2::new(30.0, y), &player.score.to_string());

                                    if *key == shared.my_player_id {
                                        if mqui::root_ui().button(
                                            mq::Vec2::new(150.0, y),
                                            if player.ready { "Unready" } else { "Ready" },
                                        ) {
                                            let new_value = !player.ready;
                                            player.ready = new_value;

                                            game_msg_send
                                                .borrow()
                                                .as_ref()
                                                .unwrap()
                                                .send(
                                                    ni_ty::protocol::GameMessageC2S::UpdateSelfReady {
                                                        value: new_value,
                                                    }
                                                    .into(),
                                                )
                                                .unwrap();
                                        }
                                    } else {
                                        mqui::root_ui().label(
                                            mq::Vec2::new(150.0, y),
                                            if player.ready { "Ready" } else { "Not Ready" },
                                        );
                                    }

                                    mqui::root_ui().label(mq::Vec2::new(450.0, y), &player.name);
                                }

                                if mqui::root_ui().button(mq::Vec2::new(10.0, 10.0), "Leave") {
                                    game_msg_send
                                        .borrow()
                                        .as_ref()
                                        .unwrap()
                                        .send(ConnectionMessage::Leave)
                                        .unwrap();
                                }
                                State::GameNeutral
                            }
                        }
                        Some(hand) => State::GameHand {
                            my_player_idx: hand
                                .players()
                                .iter()
                                .position(|player| player.player_id() == shared.my_player_id),
                        },
                    }
                } else {
                    match *lock {
                        ConnectionState::NotConnected { expected: true } => State::MainMenu,
                        _ => State::LostConnection {
                            was_connected: true,
                        },
                    }
                }
            }
            State::GameHand { my_player_idx } => {
                let mut lock = game_info_mutex.lock().unwrap();
                if let Some(shared) = (*lock).as_info_mut() {
                    if let Some(real_hand_state) = shared.game.hand.as_mut() {
                        let hand_extra = shared.hand_extra.as_mut().unwrap();

                        let player_hand_width = 130.0
                            + CARD_WIDTH
                            + (real_hand_state.players()[0].tableau_stacks().len() as f32)
                                * (CARD_WIDTH + 10.0);

                        let lake_width = ((real_hand_state.lake_stacks().len() as f32)
                            * CARD_WIDTH)
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
                        let needed_screen_height =
                            (PLAYER_Y + CARD_HEIGHT + 10.0 + CARD_HEIGHT) * 2.0;

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

                        let scale = real_screen_size.0 / screen_size.0;

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

                        let mouse_pos = mq::mouse_position();
                        let mouse_pos = mq::Vec2::new(
                            mouse_pos.0 * screen_size.0 / real_screen_size.0,
                            mouse_pos.1 * screen_size.1 / real_screen_size.1,
                        );

                        let (pred_hand_state, self_inverted) = if let Some(my_player_idx) =
                            my_player_idx
                        {
                            let my_player_idx_u8 = my_player_idx as u8;

                            let my_location = get_player_location(my_player_idx);

                            let mut pred_hand_state = (*real_hand_state).clone();
                            for action in hand_extra.pending_actions.iter() {
                                let _ = pred_hand_state.apply(Some(my_player_idx_u8), *action);
                                // ignore error, will get cleared out eventually
                            }
                            if hand_extra.self_called_nerts {
                                pred_hand_state.nerts_called = true;
                            }

                            {
                                let player_state = &pred_hand_state.players()[my_player_idx];
                                let my_position =
                                    (screen_center.0 + my_location.0, screen_center.1 + PLAYER_Y);

                                let mouse_pressed =
                                    mq::is_mouse_button_pressed(mq::MouseButton::Left);

                                if mouse_pressed
                                    || mq::is_mouse_button_released(mq::MouseButton::Left)
                                {
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

                                            if pred_hand_state
                                                .apply(Some(my_player_idx_u8), action)
                                                .is_ok()
                                            {
                                                hand_extra.pending_actions.push_back(action);
                                                game_msg_send
                                                    .borrow()
                                                    .as_ref()
                                                    .unwrap()
                                                    .send(
                                                        ni_ty::protocol::GameMessageC2S::ApplyHandAction {
                                                            action,
                                                        }.into(),
                                                    )
                                                    .unwrap();

                                                hand_extra.my_held_state = None;
                                            }
                                        }
                                    } else {
                                        let found = if player_state.nerts_stack().len() > 0
                                            && mq::Rect::new(
                                                my_position.0
                                                    + ((player_state.nerts_stack().len() - 1)
                                                        as f32)
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
                                                            + ((player_state.nerts_stack().len()
                                                                - 1)
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
                                            && player_state.waste_stack().len() > 0
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
                                                                * ((player_state
                                                                    .waste_stack()
                                                                    .len()
                                                                    .min(3)
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
                                            match hand_extra.my_held_state {
                                                None => {
                                                    if mouse_pressed {
                                                        if let (
                                                            ni_ty::StackLocation::Player(_, src),
                                                            count,
                                                            offset,
                                                        ) = found
                                                        {
                                                            let stack = pred_hand_state
                                                                .stack_at(found.0)
                                                                .unwrap();
                                                            if stack.len() > 0 {
                                                                let top_card = stack.cards()
                                                                    [stack.cards().len() - count]
                                                                    .card;

                                                                hand_extra.my_held_state =
                                                                    Some(ni_ty::HeldInfo {
                                                                        src,
                                                                        count: count as u8,
                                                                        offset: (
                                                                            offset[0], offset[1],
                                                                        ),
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
                                                            hand_extra.my_held_state = None;
                                                        }
                                                    } else {
                                                        if matches!(
                                                            target_loc,
                                                            ni_ty::StackLocation::Player(
                                                                _,
                                                                ni_ty::PlayerStackLocation::Tableau(
                                                                    _
                                                                )
                                                            ) | ni_ty::StackLocation::Lake(_)
                                                        ) {
                                                            if let Some(target_stack) =
                                                                pred_hand_state.stack_at(target_loc)
                                                            {
                                                                if let Some(src_stack) =
                                                                    pred_hand_state
                                                                        .stack_at(src_loc)
                                                                {
                                                                    let stack_cards =
                                                                        src_stack.cards();
                                                                    let back_card = &stack_cards
                                                                        [stack_cards.len()
                                                                            - held.count as usize];

                                                                    if target_stack
                                                                        .can_add(*back_card)
                                                                    {
                                                                        let action =
                                                                            ni_ty::HandAction::Move {
                                                                                from: src_loc,
                                                                                count: held.count,
                                                                                to: target_loc,
                                                                            };

                                                                        println!(
                                                                            "applying for check"
                                                                        );
                                                                        if pred_hand_state
                                                                            .apply(
                                                                                Some(my_player_idx_u8),
                                                                                action,
                                                                            )
                                                                            .is_ok()
                                                                        {
                                                                            hand_extra
                                                                                .pending_actions
                                                                                .push_back(action);
                                                                            game_msg_send.borrow().as_ref().unwrap().send(ni_ty::protocol::GameMessageC2S::ApplyHandAction { action }.into()).unwrap();
                                                                        }

                                                                        hand_extra.my_held_state =
                                                                            None;
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
                                } else if mq::is_mouse_button_pressed(mq::MouseButton::Right)
                                    || mq::is_key_pressed(mq::KeyCode::Escape)
                                {
                                    hand_extra.my_held_state = None;
                                } else if mq::is_key_pressed(mq::KeyCode::Tab) {
                                    let action = if player_state.stock_stack().len() > 0 {
                                        ni_ty::HandAction::FlipStock
                                    } else {
                                        ni_ty::HandAction::ReturnStock
                                    };

                                    if pred_hand_state
                                        .apply(Some(my_player_idx_u8), action)
                                        .is_ok()
                                    {
                                        hand_extra.pending_actions.push_back(action);
                                        game_msg_send
                                            .borrow()
                                            .as_ref()
                                            .unwrap()
                                            .send(
                                                ni_ty::protocol::GameMessageC2S::ApplyHandAction {
                                                    action,
                                                }
                                                .into(),
                                            )
                                            .unwrap();

                                        hand_extra.my_held_state = None;
                                    }
                                }
                            }

                            hand_extra.last_mouse_position = Some((
                                mouse_pos[0] - screen_center.0,
                                mouse_pos[1] - screen_center.1,
                            ));

                            (Cow::Owned(pred_hand_state), my_location.1)
                        } else {
                            (Cow::Borrowed(shared.game.hand.as_ref().unwrap()), false)
                        };
                        let _ = real_hand_state;

                        mq::clear_background(BACKGROUND_COLOR);

                        mqui::root_ui().label(
                            mq::Vec2::new(200.0, 10.0),
                            &format!(
                                "Room Code: {}",
                                to_full_game_id_str(shared.server_id, shared.game.id),
                            ),
                        );

                        for (idx, player_state) in pred_hand_state.players().iter().enumerate() {
                            let location = get_player_location(idx);
                            let position =
                                (screen_center.0 + location.0, screen_center.1 + PLAYER_Y);

                            mq::set_camera(&normal_camera);
                            if let Some(player) = shared.game.players.get(&player_state.player_id())
                            {
                                draw_text_centered(
                                    &player.name,
                                    if location.1 == self_inverted {
                                        position.0 + player_hand_width / 2.0
                                    } else {
                                        screen_center.0 - location.0 - player_hand_width / 2.0
                                    },
                                    if location.1 == self_inverted {
                                        position.1 - 20.0
                                    } else {
                                        screen_center.1 - PLAYER_Y + 20.0
                                    },
                                    40,
                                    mq::BLACK,
                                );
                            }

                            if location.1 != self_inverted {
                                mq::set_camera(&inverted_camera);
                            }

                            let held_state = if Some(idx) == my_player_idx {
                                hand_extra.my_held_state
                            } else {
                                hand_extra.mouse_states[idx]
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
                                    draw_back(
                                        position.0 + (i as f32) * 10.0,
                                        position.1,
                                        player_state.player_id(),
                                    );
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
                                            + ((player_state.nerts_stack().len() - 1) as f32)
                                                * 10.0,
                                        position.1,
                                    );
                                }
                            } else {
                                if Some(idx) == my_player_idx {
                                    if mqui::root_ui().button(
                                        mq::Vec2::new(
                                            (position.0 + CARD_WIDTH / 2.0) * scale,
                                            (position.1 + CARD_HEIGHT / 2.0) * scale,
                                        ),
                                        "Nerts!",
                                    ) {
                                        hand_extra.self_called_nerts = true;
                                        game_msg_send
                                            .borrow()
                                            .as_ref()
                                            .unwrap()
                                            .send(ni_ty::protocol::GameMessageC2S::CallNerts.into())
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
                                        if count as usize <= cards.len() {
                                            &cards[..(cards.len() - count as usize)]
                                        } else {
                                            cards
                                        }
                                    } else {
                                        cards
                                    }
                                } else {
                                    cards
                                };

                                draw_vertical_stack_cards(
                                    cards,
                                    position.0
                                        + 130.0
                                        + CARD_WIDTH
                                        + (i as f32) * (CARD_WIDTH + 10.0),
                                    position.1,
                                );
                            }

                            let stock_pos = (position.0, position.1 + CARD_HEIGHT + 10.0);
                            if player_state.stock_stack().len() > 0 {
                                draw_back(stock_pos.0, stock_pos.1, player_state.player_id());
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
                                if count as usize <= waste_cards.len() {
                                    &waste_cards[..(waste_cards.len() - count as usize)]
                                } else {
                                    waste_cards
                                }
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

                            if hand_extra.stalled {
                                mq::draw_text(
                                    "Shuffling soon if game remains stalled...",
                                    stock_pos.0,
                                    stock_pos.1 + CARD_HEIGHT + 30.0,
                                    30.0,
                                    mq::BLACK,
                                );
                            }
                        }

                        if self_inverted {
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

                        for (idx, value) in hand_extra.mouse_states.iter().enumerate() {
                            if let Some((_, state)) = value {
                                let location = get_player_location(idx);

                                if location.1 != self_inverted {
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
                                            let cards =
                                                &cards[(cards.len() - held.count as usize)..];

                                            if cards[0].card == held.top_card {
                                                draw_vertical_stack_cards(
                                                    cards,
                                                    screen_center.0 + state.position.0
                                                        - held.offset.0,
                                                    screen_center.1 + state.position.1
                                                        - held.offset.1,
                                                );
                                            }
                                        }
                                    }
                                }

                                mq::draw_texture_ex(
                                    cursors_texture,
                                    screen_center.0 + state.position.0 - 1.0,
                                    screen_center.1 + state.position.1 - 1.0,
                                    PLAYER_COLORS[(pred_hand_state.players()[idx].player_id() >> 4)
                                        as usize],
                                    mq::DrawTextureParams {
                                        source: Some(mq::Rect {
                                            x: 0.0,
                                            y: 0.0,
                                            w: 40.0,
                                            h: 80.0,
                                        }),
                                        dest_size: Some(mq::Vec2::new(20.0, 40.0)),
                                        ..Default::default()
                                    },
                                );
                            }
                        }

                        mq::set_camera(&normal_camera);

                        if let Some(my_player_idx) = my_player_idx {
                            let my_player_state = &pred_hand_state.players()[my_player_idx];
                            if let Some(ref held) = hand_extra.my_held_state {
                                let stack = my_player_state.stack_at(held.src);
                                if let Some(stack) = stack {
                                    let stack_cards = stack.cards();
                                    if stack_cards.len() >= held.count as usize {
                                        let cards = &stack_cards
                                            [(stack_cards.len() - held.count as usize)..];

                                        draw_vertical_stack_cards(
                                            cards,
                                            mouse_pos[0] - held.offset.0,
                                            mouse_pos[1] - held.offset.1,
                                        );
                                    } else {
                                        hand_extra.my_held_state = None;
                                    }
                                } else {
                                    hand_extra.my_held_state = None;
                                }
                            }
                        }

                        if pred_hand_state.nerts_called {
                            mq::draw_rectangle(
                                0.0,
                                screen_center.1 - 70.0,
                                screen_size.0,
                                140.0,
                                NERTS_OVERLAY_COLOR,
                            );

                            draw_text_centered(
                                "Nerts!",
                                screen_center.0,
                                screen_center.1,
                                100,
                                NERTS_TEXT_COLOR,
                            );
                        }

                        if mqui::root_ui().button(mq::Vec2::new(10.0, 10.0), "Leave") {
                            game_msg_send
                                .borrow()
                                .as_ref()
                                .unwrap()
                                .send(ConnectionMessage::Leave)
                                .unwrap();
                        }

                        State::GameHand { my_player_idx }
                    } else {
                        State::GameNeutral
                    }
                } else {
                    match *lock {
                        ConnectionState::NotConnected { expected: true } => State::MainMenu,
                        _ => State::LostConnection {
                            was_connected: true,
                        },
                    }
                }
            }
            State::GameEnd { scores } => {
                mq::clear_background(BACKGROUND_COLOR);

                let screen_center = (mq::screen_width() / 2.0, mq::screen_height() / 2.0);

                let mut lock = game_info_mutex.lock().unwrap();
                if let Some(shared) = (*lock).as_info_mut() {
                    match &shared.game.hand {
                        None => {
                            let row_height = 75.0;

                            let box_width = 450.0;
                            let box_height = row_height * (scores.len() as f32);

                            let box_x = screen_center.0 - box_width / 2.0;
                            let box_y = screen_center.1 - box_height / 2.0;

                            for (i, (player_id, score)) in scores.iter().enumerate() {
                                let y = box_y + (i as f32) * row_height;

                                mqui::root_ui().label(mq::Vec2::new(box_x, y), &score.to_string());

                                if let Some(player) = shared.game.players.get(&player_id) {
                                    mqui::root_ui()
                                        .label(mq::Vec2::new(box_x + 150.0, y), &player.name);
                                }
                            }

                            if mqui::widgets::Button::new("Continue")
                                .position(mq::Vec2::new(
                                    screen_center.0 - 150.0,
                                    box_y + box_height + 50.0,
                                ))
                                .size(mq::Vec2::new(300.0, 50.0))
                                .ui(&mut mqui::root_ui())
                            {
                                State::GameNeutral
                            } else {
                                State::GameEnd { scores }
                            }
                        }
                        Some(hand) => State::GameHand {
                            my_player_idx: hand
                                .players()
                                .iter()
                                .position(|player| player.player_id() == shared.my_player_id),
                        },
                    }
                } else {
                    State::LostConnection {
                        was_connected: true,
                    }
                }
            }
            State::LostConnection { was_connected } => {
                mq::clear_background(BACKGROUND_COLOR);

                let back_button_width = 300.0;
                let back_button_height = 50.0;
                let spacing = 50.0;

                let screen_center = (mq::screen_width() / 2.0, mq::screen_height() / 2.0);

                draw_text_centered(
                    if was_connected {
                        "Lost connection to the server."
                    } else {
                        "Failed to connect to the server."
                    },
                    screen_center.0,
                    screen_center.1,
                    60,
                    mq::BLACK,
                );

                if mqui::widgets::Button::new("Main Menu")
                    .position(mq::Vec2::new(
                        screen_center.0 - back_button_width / 2.0,
                        screen_center.1 + spacing,
                    ))
                    .size(mq::Vec2::new(back_button_width, back_button_height))
                    .ui(&mut mqui::root_ui())
                {
                    State::MainMenu
                } else {
                    State::LostConnection { was_connected }
                }
            }
        };

        mq::next_frame().await
    }
}
