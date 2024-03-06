use futures_util::{FutureExt, StreamExt};
use macroquad::prelude as mq;
use nertsio_types as ni_ty;
use nertsio_ui_metrics as metrics;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::future::Future;
use std::sync::{Arc, Mutex};

mod connection;

use connection::{ConnectionEvent, ConnectionMessage};

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

const GAME_ID_FORMAT: u128 = lexical::NumberFormatBuilder::from_radix(36);

const COORDINATOR_URL: &str = "https://coordinator.nerts.io/";
// const COORDINATOR_URL: &str = "http://localhost:6462/";

const MAX_INTERPOLATION_TIME: f32 = 0.3;

const SCREEN_MARGIN: f32 = 2.5;

fn default_name() -> String {
    "Nerter".to_owned()
}

#[derive(Clone, PartialEq, Deserialize, Serialize)]
struct Settings {
    #[serde(default = "default_name")]
    name: String,

    #[serde(default)]
    drag: bool,

    #[serde(default)]
    round_start_music: bool,

    #[serde(default)]
    suit_callouts: bool,

    #[serde(default)]
    nerts_callout: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            name: default_name(),
            drag: false,
            round_start_music: false,
            suit_callouts: false,
            nerts_callout: false,
        }
    }
}

enum ConnectionState {
    NotConnected { expected: bool, code: Option<u8> },
    Connecting,
    Connected(SharedInfo),
}

impl ConnectionState {
    pub fn as_info_mut(&mut self) -> Option<&mut SharedInfo> {
        match self {
            ConnectionState::NotConnected { .. } | ConnectionState::Connecting => None,
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

#[derive(Clone)]
struct MouseState {
    seq: u32,
    inner: ni_ty::MouseState,
    current_animation: Option<(splines::Spline<f32, mq::Vec2>, f32)>,
    time_since_update: f32,
}

impl MouseState {
    pub fn new(seq: u32, inner: ni_ty::MouseState) -> Self {
        Self {
            seq,
            inner,
            current_animation: None,
            time_since_update: 0.0,
        }
    }

    pub fn get_pos(&self) -> mq::Vec2 {
        self.current_animation
            .as_ref()
            .and_then(|(spline, time)| spline.sample(*time))
            .unwrap_or_else(|| self.inner.position.into())
    }

    pub fn step(&mut self, delta: f32) {
        if let Some((spline, ref mut time)) = &mut self.current_animation {
            *time += delta;
            if *time >= spline.keys().last().unwrap().t {
                self.current_animation = None;
            }
        }

        self.time_since_update += delta;
    }

    pub fn receive(&mut self, seq: u32, inner: ni_ty::MouseState) {
        if self.seq < seq {
            let duration = (self.time_since_update * 0.9).min(MAX_INTERPOLATION_TIME);
            match &mut self.current_animation {
                Some((ref mut spline, time)) => {
                    let t = spline.keys().last().unwrap().t + duration;
                    log::debug!("adding new point at {} (current {})", t, time);
                    spline.add(splines::Key::new(
                        t,
                        inner.position.into(),
                        splines::Interpolation::Cosine,
                    ));
                }
                None => {
                    self.current_animation = Some((
                        splines::Spline::from_vec(vec![
                            splines::Key::new(
                                0.0,
                                self.inner.position.into(),
                                splines::Interpolation::Cosine,
                            ),
                            splines::Key::new(
                                duration,
                                inner.position.into(),
                                splines::Interpolation::Cosine,
                            ),
                        ]),
                        0.0,
                    ));
                }
            }

            self.time_since_update = 0.0;
            self.inner = inner;
            self.seq = seq;
        }
    }
}

struct HeldState {
    info: ni_ty::HeldInfo,
    mouse_released: bool,
}

struct HandExtra {
    expected_start_time: Option<web_time::Instant>,
    pending_actions: VecDeque<ni_ty::HandAction>,
    self_called_nerts: bool,
    mouse_states: Vec<Option<MouseState>>,
    my_held_state: Option<HeldState>,
    last_mouse_position: Option<(f32, f32)>,
    stalled: bool,
}

impl HandExtra {
    pub fn new(player_count: usize) -> Self {
        Self {
            expected_start_time: None,
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
    MainMenu {
        show_settings: bool,
    },
    JoinGameForm {
        input: String,
    },
    PublicGameListLoading {
        channel: futures_channel::oneshot::Receiver<
            Vec<ni_ty::protocol::PublicGameInfoExpanded<'static>>,
        >,
    },
    PublicGameList {
        list: Vec<ni_ty::protocol::PublicGameInfoExpanded<'static>>,
    },
    Connecting,
    GameNeutral {
        show_settings: bool,
    },
    GameHand {
        my_player_idx: Option<usize>,
        show_settings: bool,
    },
    GameEnd {
        scores: Vec<(u8, i32)>,
    },
    LostConnection {
        was_connected: bool,
        code: Option<u8>,
    },
}

impl State {
    pub const MAIN_MENU: State = State::MainMenu {
        show_settings: false,
    };

    pub const GAME_NEUTRAL: State = State::GameNeutral {
        show_settings: false,
    };

    pub fn from_connection_state(src: &ConnectionState) -> Self {
        match src {
            ConnectionState::NotConnected {
                expected: true,
                code: _,
            } => State::MAIN_MENU,
            ConnectionState::NotConnected {
                expected: false,
                code,
            } => State::LostConnection {
                was_connected: true,
                code: *code,
            },
            _ => State::LostConnection {
                was_connected: true,
                code: None,
            },
        }
    }
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

#[cfg(target_family = "wasm")]
const SETTINGS_KEY: &str = "nertsioSettings";

#[cfg(target_family = "wasm")]
async fn run_settings_save_loop(
    storage: web_sys::Storage,
    init_value: Settings,
    mutex: Arc<Mutex<Settings>>,
) {
    log::debug!("run_settings_save_loop");

    let mut saved_value = init_value;

    let mut interval = futures_ticker::Ticker::new(std::time::Duration::from_secs(5));

    loop {
        interval.next().await;

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
                let buf = serde_json::to_string(&saved_value)?;

                storage
                    .set_item(SETTINGS_KEY, &buf)
                    .map_err(|err| anyhow::anyhow!("Failed to set item: {:?}", err))
            }
            .await
            {
                log::error!("failed to save settings: {:?}", err);
            }
        }
    }
}

#[cfg(not(target_family = "wasm"))]
async fn run_settings_save_loop(
    config_path: std::path::PathBuf,
    init_value: Settings,
    mutex: Arc<Mutex<Settings>>,
) {
    let mut saved_value = init_value;

    let file = Arc::new(atomicwrites::AtomicFile::new(
        config_path,
        atomicwrites::OverwriteBehavior::AllowOverwrite,
    ));

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
                let file = file.clone();
                tokio::task::spawn_blocking(move || {
                    file.write(|f| {
                        use std::io::Write;

                        f.write_all(&buf)
                    })?;

                    Result::<_, anyhow::Error>::Ok(())
                })
                .await?
            }
            .await
            {
                log::error!("failed to save settings: {:?}", err);
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

#[cfg(target_family = "wasm")]
type AsyncRt = WasmAsyncRt;

#[cfg(not(target_family = "wasm"))]
type AsyncRt = tokio::runtime::Handle;

#[cfg(target_family = "wasm")]
#[derive(Clone)]
struct WasmAsyncRt;
#[cfg(target_family = "wasm")]
impl WasmAsyncRt {
    pub fn spawn<F: Future<Output = ()> + 'static>(&self, fut: F) {
        wasm_bindgen_futures::spawn_local(fut);
    }

    pub fn handle(&self) -> &Self {
        &self
    }
}

fn render_settings_window(egui_ctx: &egui::Context, settings_mutex: &Mutex<Settings>) -> bool {
    let menu_width = 300.0;

    let mut open = true;

    egui::containers::Window::new("Settings")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .open(&mut open)
        .show(egui_ctx, |ui| {
            let mut settings_lock = settings_mutex.lock().unwrap();
            let settings = &mut *settings_lock;

            ui.vertical_centered(|ui| {
                ui.allocate_ui_with_layout(
                    egui::Vec2::new(menu_width, 0.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.checkbox(&mut settings.drag, "Allow Drag-and-Drop");
                        ui.checkbox(&mut settings.round_start_music, "Round Start Music");
                        ui.checkbox(&mut settings.suit_callouts, "Suit Callouts");
                        ui.checkbox(&mut settings.nerts_callout, "Nerts Callout");
                    },
                );
            });
        });

    open
}

#[macroquad::main(get_window_conf)]
async fn main() {
    #[cfg(not(target_family = "wasm"))]
    {
        env_logger::init_from_env(
            env_logger::Env::default()
                .filter_or(env_logger::DEFAULT_FILTER_ENV, "nertsio_ui=debug"),
        );
    }
    #[cfg(target_family = "wasm")]
    {
        std::panic::set_hook(Box::new(console_error_panic_hook::hook));
        wasm_logger::init(wasm_logger::Config::default());
    }

    #[cfg(not(target_family = "wasm"))]
    let async_rt = tokio::runtime::Runtime::new().unwrap();

    #[cfg(target_family = "wasm")]
    let async_rt = WasmAsyncRt;

    let card_size = mq::Vec2::new(metrics::CARD_WIDTH, metrics::CARD_HEIGHT);

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

    let round_start_music =
        macroquad::audio::load_sound_from_bytes(include_bytes!("../res/nertson.ogg"))
            .await
            .unwrap();

    let suit_callout_spades =
        macroquad::audio::load_sound_from_bytes(include_bytes!("../res/spades.ogg"))
            .await
            .unwrap();

    let suit_callout_diamonds =
        macroquad::audio::load_sound_from_bytes(include_bytes!("../res/diamonds.ogg"))
            .await
            .unwrap();

    let suit_callout_clubs =
        macroquad::audio::load_sound_from_bytes(include_bytes!("../res/clubs.ogg"))
            .await
            .unwrap();

    let suit_callout_hearts =
        macroquad::audio::load_sound_from_bytes(include_bytes!("../res/hearts.ogg"))
            .await
            .unwrap();

    let nerts_callout = macroquad::audio::load_sound_from_bytes(include_bytes!("../res/nerts.ogg"))
        .await
        .unwrap();

    let draw_card = |card: ni_ty::Card, x: f32, y: f32| {
        mq::draw_texture_ex(
            &cards_texture,
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
            &backs_texture,
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
            &backs_texture,
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
            &placeholder_texture,
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
                draw_card(
                    card.card,
                    x,
                    y + (i as f32) * metrics::VERTICAL_STACK_SPACING,
                );
            }
        }
    };

    let draw_horizontal_stack_cards = |cards: &[ni_ty::CardInstance], x: f32, y: f32| {
        if cards.is_empty() {
            mq::draw_texture_ex(
                &placeholder_texture,
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
                draw_card(
                    card.card,
                    x + (i as f32) * metrics::HORIZONTAL_STACK_SPACING,
                    y,
                );
            }
        }
    };

    let game_info_mutex = Arc::new(std::sync::Mutex::new(ConnectionState::NotConnected {
        expected: true,
        code: None,
    }));
    let game_msg_send = RefCell::new(None);

    let (events_send, mut events_recv) = futures_channel::mpsc::unbounded();

    let http_client = reqwest::Client::new();

    let settings_mutex;
    #[cfg(not(target_family = "wasm"))]
    {
        let config_dir = dirs::config_dir()
            .map(Cow::Owned)
            .unwrap_or_else(|| std::path::Path::new(".").into());

        let config_path = config_dir.join("nertsio.json");

        match std::fs::File::open(&config_path) {
            Ok(mut file) => {
                let init_value: Settings = match serde_json::from_reader(&mut file) {
                    Ok(value) => value,
                    Err(err) => {
                        log::debug!("Failed to parse config file: {:?}", err);
                        log::debug!("Will reset config to defaults.");

                        Default::default()
                    }
                };

                settings_mutex = Arc::new(Mutex::new(init_value.clone()));
                async_rt.spawn(run_settings_save_loop(
                    config_path,
                    init_value,
                    settings_mutex.clone(),
                ));
            }
            Err(err) => {
                if err.kind() == std::io::ErrorKind::NotFound {
                    let init_value: Settings = Default::default();

                    settings_mutex = Arc::new(Mutex::new(init_value.clone()));
                    async_rt.spawn(run_settings_save_loop(
                        config_path,
                        init_value,
                        settings_mutex.clone(),
                    ));
                } else {
                    log::error!("Failed to open settings file: {:?}", err);
                    log::error!("Settings will not be saved.");

                    settings_mutex = Arc::new(Mutex::new(Default::default()));
                }
            }
        }
    }
    #[cfg(target_family = "wasm")]
    {
        match web_sys::window()
            .ok_or_else(|| anyhow::anyhow!("Can't access window"))
            .and_then(|window| {
                window
                    .local_storage()
                    .map_err(|err| anyhow::anyhow!("Can't access localStorage: {:?}", err))
                    .and_then(|x| x.ok_or_else(|| anyhow::anyhow!("Can't access localStorage")))
            }) {
            Ok(storage) => {
                let init_value: Settings = match storage.get_item(SETTINGS_KEY) {
                    Ok(None) => Default::default(),
                    Ok(Some(buf)) => match serde_json::from_str(&buf) {
                        Ok(value) => value,
                        Err(err) => {
                            log::debug!("Failed to parse config file: {:?}", err);
                            log::debug!("Will reset config to defaults.");

                            Default::default()
                        }
                    },
                    Err(err) => {
                        log::debug!("Failed to fetch config file: {:?}", err);
                        log::debug!("Will reset config to defaults.");

                        Default::default()
                    }
                };

                settings_mutex = Arc::new(Mutex::new(init_value.clone()));
                async_rt.spawn(run_settings_save_loop(
                    storage,
                    init_value,
                    settings_mutex.clone(),
                ));
            }
            Err(err) => {
                log::error!("Failed to init settings: {:?}", err);
                log::error!("Settings will not be saved.");

                settings_mutex = Arc::new(Mutex::new(Default::default()));
            }
        }
    }

    let do_connection = |connection_type| {
        let (new_game_msg_send, game_msg_recv) = futures_channel::mpsc::unbounded();
        *game_msg_send.borrow_mut() = Some(new_game_msg_send);

        let handle = async_rt.handle().clone();

        async_rt.spawn({
            let game_info_mutex = game_info_mutex.clone();
            let http_client = http_client.clone();
            let settings_mutex = settings_mutex.clone();
            let events_send = events_send.clone();
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
                    events_send,
                    handle,
                )
                .await;

                let mut lock = game_info_mutex.lock().unwrap();
                if let Err(err) = res {
                    (*lock) = ConnectionState::NotConnected {
                        expected: false,
                        code: Some(0), // TODO?
                    };
                    log::error!("Failed to handle connection: {:?}", err);
                } else {
                    (*lock) = ConnectionState::NotConnected {
                        expected: true,
                        code: None,
                    };
                }
            }
        });
    };

    let start_loading_public_games = || {
        let (send, recv) = futures_channel::oneshot::channel();

        let req_fut = http_client
            .get(format!("{}public_games", COORDINATOR_URL))
            .send();
        async_rt.spawn(
            (async move {
                let resp = req_fut.await?.error_for_status()?;

                let resp: ni_ty::protocol::RespList<ni_ty::protocol::PublicGameInfoExpanded> =
                    resp.json().await?;

                let _ = send.send(resp.items); // if this fails, then we didn't need it anyway

                Result::<_, anyhow::Error>::Ok(())
            })
            .then(|res| {
                if let Err(err) = res {
                    log::error!("Failed to list public games: {:?}", err);
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

    let mut state = State::MAIN_MENU;

    egui_macroquad::cfg(|egui_ctx| {
        egui_ctx.set_visuals(egui::style::Visuals::light());
        egui_ctx.set_zoom_factor(3.0);
    });

    let mut quit = false;

    while !quit {
        mq::set_default_camera();

        state = match state {
            State::MainMenu { show_settings } => {
                mq::clear_background(BACKGROUND_COLOR);

                let button_height = 20.0;

                let button_count = 7;

                let menu_width = 150.0;

                let mut next_state = State::MainMenu { show_settings };

                egui_macroquad::ui(|egui_ctx| {
                    egui::CentralPanel::default()
                        .frame(egui::Frame::none())
                        .show(egui_ctx, |ui| {
                            ui.set_enabled(!show_settings);

                            let ui_screen_width = mq::screen_width() / egui_ctx.zoom_factor();
                            let ui_screen_height = mq::screen_height() / egui_ctx.zoom_factor();

                            let menu_height = button_height * (button_count as f32)
                                + ((button_count - 1) as f32) * ui.spacing().item_spacing.y;

                            let menu_x = ui_screen_width / 2.0 - menu_width / 2.0;
                            let menu_y = ui_screen_height / 2.0 - menu_height / 2.0;

                            ui.allocate_ui_at_rect(
                                egui::Rect {
                                    min: egui::Pos2::new(menu_x, menu_y),
                                    max: egui::Pos2::new(menu_x + menu_width, menu_y + menu_height),
                                },
                                |ui| {
                                    ui.with_layout(
                                        egui::Layout::top_down(egui::Align::Center),
                                        |ui| {
                                            ui.heading("nertsio");
                                        },
                                    );

                                    let menu_button =
                                        |ui: &mut egui::Ui, label| {
                                            ui.add(egui::widgets::Button::new(label).min_size(
                                                egui::Vec2::new(menu_width, button_height),
                                            ))
                                            .clicked()
                                        };

                                    {
                                        let mut settings_lock = settings_mutex.lock().unwrap();
                                        let settings = &mut *settings_lock;

                                        ui.horizontal(|ui| {
                                            ui.label("Name:");
                                            ui.text_edit_singleline(&mut settings.name);
                                        });
                                    }

                                    next_state = if menu_button(ui, "Create Public Game") {
                                        do_connection(connection::ConnectionType::CreateGame {
                                            public: true,
                                        });
                                        State::Connecting
                                    } else if menu_button(ui, "Create Private Game") {
                                        do_connection(connection::ConnectionType::CreateGame {
                                            public: false,
                                        });
                                        State::Connecting
                                    } else if menu_button(ui, "Join Public Game") {
                                        let channel = start_loading_public_games();
                                        State::PublicGameListLoading { channel }
                                    } else if menu_button(ui, "Join Private Game") {
                                        State::JoinGameForm {
                                            input: String::new(),
                                        }
                                    } else if menu_button(ui, "Settings") {
                                        State::MainMenu {
                                            show_settings: true,
                                        }
                                    } else if cfg!(not(target_family = "wasm"))
                                        && menu_button(ui, "Quit")
                                    {
                                        quit = true;
                                        State::MainMenu { show_settings }
                                    } else {
                                        State::MainMenu { show_settings }
                                    };
                                },
                            );
                        });

                    if show_settings {
                        if !render_settings_window(&egui_ctx, &settings_mutex) {
                            next_state = State::MainMenu {
                                show_settings: false,
                            };
                        }
                    }
                });

                egui_macroquad::draw();

                next_state
            }
            State::JoinGameForm { mut input } => {
                let menu_width = 150.0;

                let button_height = 20.0;
                let button_count = 2;

                mq::clear_background(BACKGROUND_COLOR);

                let mut go_connect = false;
                let mut go_back = false;

                egui_macroquad::ui(|egui_ctx| {
                    egui::CentralPanel::default()
                        .frame(
                            egui::Frame::none()
                                .inner_margin(egui::style::Margin::same(SCREEN_MARGIN)),
                        )
                        .show(egui_ctx, |ui| {
                            let ui_screen_width = mq::screen_width() / egui_ctx.zoom_factor();
                            let ui_screen_height = mq::screen_height() / egui_ctx.zoom_factor();

                            let menu_height = button_height * (button_count as f32)
                                + ((button_count - 1) as f32) * ui.spacing().item_spacing.y;

                            let menu_x = ui_screen_width / 2.0 - menu_width / 2.0;
                            let menu_y = ui_screen_height / 2.0 - menu_height / 2.0;

                            if ui.button("Back").clicked() {
                                go_back = true;
                            }

                            ui.allocate_ui_at_rect(
                                egui::Rect {
                                    min: egui::Pos2::new(menu_x, menu_y),
                                    max: egui::Pos2::new(menu_x + menu_width, menu_y + menu_height),
                                },
                                |ui| {
                                    ui.horizontal(|ui| {
                                        ui.label("Room Code:");
                                        ui.text_edit_singleline(&mut input);
                                    });

                                    ui.vertical_centered(|ui| {
                                        if ui.button("Join").clicked() {
                                            go_connect = true;
                                        }
                                    });
                                },
                            );
                        });
                });

                egui_macroquad::draw();

                go_back = go_back || mq::is_key_pressed(mq::KeyCode::Escape);

                if go_connect {
                    if let Ok((server_id, game_id)) = parse_full_game_id_str(&input) {
                        do_connection(connection::ConnectionType::JoinPrivateGame {
                            server_id,
                            game_id,
                        });
                        State::Connecting
                    } else {
                        State::JoinGameForm { input }
                    }
                } else if go_back {
                    State::MAIN_MENU
                } else {
                    State::JoinGameForm { input }
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
                    State::MAIN_MENU
                } else {
                    match channel.try_recv() {
                        Ok(Some(list)) => State::PublicGameList { list },
                        Ok(None) => State::PublicGameListLoading { channel },
                        Err(futures_channel::oneshot::Canceled) => State::MAIN_MENU,
                    }
                }
            }
            State::PublicGameList { list } => {
                mq::clear_background(BACKGROUND_COLOR);

                let screen_center = (mq::screen_width() / 2.0, mq::screen_height() / 2.0);

                let menu_width = 250.0;

                let mut go_back = false;

                let mut joining = None;

                egui_macroquad::ui(|egui_ctx| {
                    egui::CentralPanel::default()
                        .frame(
                            egui::Frame::none()
                                .inner_margin(egui::style::Margin::same(SCREEN_MARGIN)),
                        )
                        .show(egui_ctx, |ui| {
                            if ui.button("Back").clicked() {
                                go_back = true;
                            }

                            if !list.is_empty() {
                                ui.vertical_centered(|ui| {
                                    ui.heading("Public Games");
                                    ui.allocate_ui_with_layout(
                                        egui::Vec2::new(menu_width, 0.0),
                                        egui::Layout::top_down(egui::Align::Min),
                                        |ui| {
                                            egui::Grid::new("public_game_list").show(ui, |ui| {
                                                for game in &list {
                                                    ui.label(to_full_game_id_str(
                                                        game.server.server_id,
                                                        game.game_id,
                                                    ));
                                                    ui.label(format!("{} players", game.players));
                                                    ui.label(if game.waiting {
                                                        "waiting"
                                                    } else {
                                                        "playing"
                                                    });
                                                    if ui.button("Join").clicked() {
                                                        joining = Some(game);
                                                    }

                                                    ui.end_row();
                                                }
                                            });
                                        },
                                    );
                                });
                            }
                        });
                });

                egui_macroquad::draw();

                if list.is_empty() {
                    draw_text_centered(
                        "No games found.",
                        screen_center.0,
                        screen_center.1,
                        50,
                        mq::BLACK,
                    );
                }

                go_back = go_back || mq::is_key_pressed(mq::KeyCode::Escape);

                match joining {
                    None => {
                        if go_back {
                            State::MAIN_MENU
                        } else {
                            State::PublicGameList { list }
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
                        .unbounded_send(ConnectionMessage::Leave)
                        .unwrap();
                }

                match *game_info_mutex.lock().unwrap() {
                    ConnectionState::Connecting => State::Connecting,
                    ConnectionState::Connected(_) => State::GAME_NEUTRAL,
                    ConnectionState::NotConnected { expected, code } => {
                        if expected {
                            State::MAIN_MENU
                        } else {
                            State::LostConnection {
                                was_connected: false,
                                code,
                            }
                        }
                    }
                }
            }
            State::GameNeutral { show_settings } => {
                mq::clear_background(BACKGROUND_COLOR);

                let mut lock = game_info_mutex.lock().unwrap();
                if let Some(shared) = (*lock).as_info_mut() {
                    match &shared.game.hand {
                        None => {
                            if let Some(scores) = shared.new_end_scores.take() {
                                State::GameEnd { scores }
                            } else {
                                let sorted = {
                                    let mut result: Vec<u8> =
                                        shared.game.players.keys().copied().collect();
                                    result.sort_by_key(|key| -shared.game.players[key].score);
                                    result
                                };

                                let mut next_state = State::GameNeutral { show_settings };

                                egui_macroquad::ui(|egui_ctx| {
                                    egui::CentralPanel::default().frame(egui::Frame::none().inner_margin(egui::style::Margin::same(SCREEN_MARGIN))).show(egui_ctx, |ui| {
                                        ui.set_enabled(!show_settings);

                                        ui.with_layout(egui::Layout::left_to_right(egui::Align::Min), |ui| {
                                            if ui.button("Leave").clicked() {
                                                game_msg_send
                                                    .borrow()
                                                    .as_ref()
                                                    .unwrap()
                                                    .unbounded_send(ConnectionMessage::Leave)
                                                    .unwrap();
                                            }

                                            if ui.button("Settings").clicked() {
                                                next_state = State::GameNeutral { show_settings: true };
                                            }
                                        });

                                        ui.label(format!("Room Code: {}", to_full_game_id_str(shared.server_id, shared.game.id)));

                                        egui::Grid::new("scoreboard_grid").show(ui, |ui| {
                                            for key in sorted.iter() {
                                                let player = shared.game.players.get_mut(key).unwrap();

                                                ui.label(player.score.to_string());

                                                if *key == shared.my_player_id {
                                                    if ui.button(if player.ready { "Unready" } else { "Ready" }).clicked() {
                                                        let new_value = !player.ready;
                                                        player.ready = new_value;

                                                        game_msg_send
                                                            .borrow()
                                                            .as_ref()
                                                            .unwrap()
                                                            .unbounded_send(
                                                                ni_ty::protocol::GameMessageC2S::UpdateSelfReady {
                                                                    value: new_value,
                                                                }
                                                                .into(),
                                                            )
                                                            .unwrap();
                                                    }
                                                } else {
                                                    ui.label(if player.ready { "Ready" } else { "Not Ready" });
                                                }

                                                ui.horizontal(|ui| {
                                                    if shared.game.master_player == *key {
                                                        ui.colored_label(egui::Color32::YELLOW, "★");
                                                    } else if shared.game.master_player == shared.my_player_id {
                                                        if ui.button("x").clicked() {
                                                            game_msg_send
                                                                .borrow()
                                                                .as_ref()
                                                                .unwrap()
                                                                .unbounded_send(
                                                                    ni_ty::protocol::GameMessageC2S::KickPlayer {
                                                                        player: *key,
                                                                    }
                                                                    .into(),
                                                                )
                                                                .unwrap();
                                                        }
                                                    }
                                                    ui.label(&player.name);
                                                });

                                                ui.end_row();
                                            }
                                        });

                                        if shared.game.master_player == shared.my_player_id {
                                            if ui.button("Add Bot").clicked() {
                                                game_msg_send
                                                    .borrow()
                                                    .as_ref()
                                                    .unwrap()
                                                    .unbounded_send(
                                                        ni_ty::protocol::GameMessageC2S::AddBot.into(),
                                                    )
                                                    .unwrap();
                                            }
                                        }

                                    });

                                    if show_settings {
                                        if !render_settings_window(&egui_ctx, &settings_mutex) {
                                            next_state = State::GameNeutral {
                                                show_settings: false,
                                            };
                                        }
                                    }
                                });

                                egui_macroquad::draw();

                                next_state
                            }
                        }
                        Some(hand) => State::GameHand {
                            my_player_idx: hand
                                .players()
                                .iter()
                                .position(|player| player.player_id() == shared.my_player_id),
                            show_settings,
                        },
                    }
                } else {
                    State::from_connection_state(&*lock)
                }
            }
            State::GameHand {
                my_player_idx,
                show_settings,
            } => {
                let interaction_enabled = !show_settings;

                let mut lock = game_info_mutex.lock().unwrap();
                if let Some(shared) = (*lock).as_info_mut() {
                    if let Some(real_hand_state) = shared.game.hand.as_mut() {
                        let started = real_hand_state.started;
                        let hand_extra = shared.hand_extra.as_mut().unwrap();

                        let metrics = metrics::HandMetrics::new(
                            real_hand_state.players().len(),
                            real_hand_state.players()[0].tableau_stacks().len(),
                            real_hand_state.lake_stacks().len(),
                        );

                        let needed_screen_width = metrics.needed_screen_width();
                        let needed_screen_height = metrics.needed_screen_height();

                        let real_screen_size = (mq::screen_width(), mq::screen_height());
                        let screen_size = if real_screen_size.0 > needed_screen_width * 2.0
                            && real_screen_size.1 > needed_screen_height * 2.0
                        {
                            (real_screen_size.0 / 2.0, real_screen_size.1 / 2.0)
                        } else if real_screen_size.0 > needed_screen_width * 1.5
                            && real_screen_size.1 > needed_screen_height * 1.5
                        {
                            (real_screen_size.0 / 1.5, real_screen_size.1 / 1.5)
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

                        let camera_rect =
                            mq::Rect::new(0.0, screen_size.1, screen_size.0, -screen_size.1).into();

                        let normal_camera = mq::Camera2D {
                            ..mq::Camera2D::from_display_rect(camera_rect)
                        };

                        let inverted_camera = mq::Camera2D {
                            rotation: 180.0,
                            ..mq::Camera2D::from_display_rect(camera_rect)
                        };

                        let screen_center = (screen_size.0 / 2.0, screen_size.1 / 2.0);

                        let mouse_pos = mq::mouse_position();
                        let mouse_pos = mq::Vec2::new(
                            mouse_pos.0 * screen_size.0 / real_screen_size.0,
                            mouse_pos.1 * screen_size.1 / real_screen_size.1,
                        );

                        let (pred_hand_state, self_inverted) = if let Some(my_player_idx) =
                            my_player_idx
                        {
                            let my_player_idx_u8 = my_player_idx as u8;

                            let my_location = metrics.player_loc(my_player_idx);

                            let mut pred_hand_state = (*real_hand_state).clone();
                            for action in hand_extra.pending_actions.iter() {
                                let _ = pred_hand_state.apply(Some(my_player_idx_u8), *action);
                                // ignore error, will get cleared out eventually
                            }
                            if hand_extra.self_called_nerts {
                                pred_hand_state.nerts_called = true;
                            }

                            if started && interaction_enabled {
                                let player_state = &pred_hand_state.players()[my_player_idx];

                                let mouse_pressed =
                                    mq::is_mouse_button_pressed(mq::MouseButton::Left);

                                let mut settings_lock = settings_mutex.lock().unwrap();
                                let settings = &mut *settings_lock;

                                if mouse_pressed
                                    || (mq::is_mouse_button_released(mq::MouseButton::Left)
                                        && settings.drag)
                                {
                                    let nerts_stack_pos = mq::Vec2::from(metrics.player_stack_pos(
                                        ni_ty::PlayerStackLocation::Nerts,
                                        my_location,
                                    )) + mq::Vec2::from(screen_center);
                                    let stock_stack_pos = mq::Vec2::from(metrics.player_stack_pos(
                                        ni_ty::PlayerStackLocation::Stock,
                                        my_location,
                                    )) + mq::Vec2::from(screen_center);
                                    let waste_stack_pos = mq::Vec2::from(metrics.player_stack_pos(
                                        ni_ty::PlayerStackLocation::Waste,
                                        my_location,
                                    )) + mq::Vec2::from(screen_center);

                                    if mq::Rect::new(
                                        stock_stack_pos[0],
                                        stock_stack_pos[1],
                                        metrics::CARD_WIDTH,
                                        metrics::CARD_HEIGHT,
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
                                                    .unbounded_send(
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
                                                nerts_stack_pos[0]
                                                    + ((player_state.nerts_stack().len() - 1)
                                                        as f32)
                                                        * metrics::HORIZONTAL_STACK_SPACING,
                                                nerts_stack_pos[1],
                                                metrics::CARD_WIDTH,
                                                metrics::CARD_HEIGHT,
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
                                                        nerts_stack_pos[0]
                                                            + ((player_state.nerts_stack().len()
                                                                - 1)
                                                                as f32)
                                                                * metrics::HORIZONTAL_STACK_SPACING,
                                                        nerts_stack_pos[1],
                                                    ),
                                            ))
                                        } else if mq::Rect::new(
                                            screen_center.0 + metrics.lake_start_x(),
                                            screen_center.1 - metrics::CARD_HEIGHT / 2.0,
                                            metrics.lake_width(),
                                            metrics::CARD_HEIGHT,
                                        )
                                        .contains(mouse_pos)
                                        {
                                            let stack_idx_for_me = ((mouse_pos[0]
                                                - (screen_center.0 + metrics.lake_start_x()))
                                                / (metrics::CARD_WIDTH + metrics::LAKE_SPACING))
                                                as u16;

                                            let stack_idx = if my_location.inverted {
                                                (pred_hand_state.lake_stacks().len() as u16)
                                                    - stack_idx_for_me
                                                    - 1
                                            } else {
                                                stack_idx_for_me
                                            };

                                            let loc = ni_ty::StackLocation::Lake(stack_idx);
                                            let stack_pos = mq::Vec2::from(metrics.stack_pos(loc))
                                                + mq::Vec2::from(screen_center);

                                            Some((loc, 1, mouse_pos - stack_pos))
                                        } else if mq::Rect::new(
                                            waste_stack_pos[0],
                                            waste_stack_pos[1],
                                            metrics::CARD_WIDTH
                                                + metrics::HORIZONTAL_STACK_SPACING * 2.0,
                                            metrics::CARD_HEIGHT,
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
                                                        waste_stack_pos[0]
                                                            + (metrics::HORIZONTAL_STACK_SPACING
                                                                * ((player_state
                                                                    .waste_stack()
                                                                    .len()
                                                                    .min(3)
                                                                    - 1)
                                                                    as f32)),
                                                        waste_stack_pos[1],
                                                    ),
                                            ))
                                        } else {
                                            player_state
                                                .tableau_stacks()
                                                .iter()
                                                .enumerate()
                                                .filter_map(|(i, stack)| {
                                                    let loc = ni_ty::PlayerStackLocation::Tableau(i as u8);

                                                    let stack_pos = mq::Vec2::from(metrics.player_stack_pos(loc, my_location)) + mq::Vec2::from(screen_center);
                                                    if mq::Rect::new(
                                                        stack_pos[0],
                                                        stack_pos[1],
                                                        metrics::CARD_WIDTH,
                                                        metrics::CARD_HEIGHT
                                                            + ((stack.len() as f32) - 1.0)
                                                                * metrics::VERTICAL_STACK_SPACING,
                                                    )
                                                    .contains(mouse_pos)
                                                    {
                                                        let loc = ni_ty::StackLocation::Player(
                                                            my_player_idx_u8,
                                                            ni_ty::PlayerStackLocation::Tableau(i as u8),
                                                        );
                                                        if stack.len() > 0 {
                                                            let found_idx = (((mouse_pos[1]
                                                                - stack_pos[1])
                                                                / metrics::VERTICAL_STACK_SPACING)
                                                                as usize)
                                                                .min(stack.len() - 1);

                                                            Some((
                                                                loc,
                                                                stack.len() - found_idx,
                                                                mouse_pos
                                                                    - mq::Vec2::new(
                                                                        stack_pos[0],
                                                                        stack_pos[1]
                                                                            + ((found_idx as f32)
                                                                                * metrics::VERTICAL_STACK_SPACING),
                                                                    ),
                                                            ))
                                                        } else {
                                                            Some((
                                                                loc,
                                                                0,
                                                                mouse_pos - mq::Vec2::from(stack_pos),
                                                            ))
                                                        }
                                                    } else {
                                                        None
                                                    }
                                                })
                                                .next()
                                        };

                                        let _ = player_state;

                                        log::debug!("click found {:?}", found);

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
                                                                    Some(HeldState {
                                                                        info: ni_ty::HeldInfo {
                                                                            src,
                                                                            count: count as u8,
                                                                            offset: (
                                                                                offset[0],
                                                                                offset[1],
                                                                            ),
                                                                            top_card,
                                                                        },
                                                                        mouse_released: false,
                                                                    })
                                                            }
                                                        }
                                                    }
                                                }
                                                Some(ref mut held) => {
                                                    let src_loc = ni_ty::StackLocation::Player(
                                                        my_player_idx_u8,
                                                        held.info.src,
                                                    );

                                                    let (target_loc, ..) = found;
                                                    if target_loc == src_loc {
                                                        if mouse_pressed {
                                                            hand_extra.my_held_state = None;
                                                        } else {
                                                            held.mouse_released = true;
                                                        }
                                                    } else {
                                                        let success = if matches!(
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
                                                                            - held.info.count
                                                                                as usize];

                                                                    if target_stack
                                                                        .can_add(*back_card)
                                                                    {
                                                                        let action =
                                                                            ni_ty::HandAction::Move {
                                                                                from: src_loc,
                                                                                count: held.info.count,
                                                                                to: target_loc,
                                                                            };

                                                                        log::debug!(
                                                                            "applying for check"
                                                                        );
                                                                        if pred_hand_state
                                                                            .apply(
                                                                                Some(my_player_idx_u8),
                                                                                action,
                                                                            )
                                                                            .is_ok()
                                                                        {
                                                                            // should always be
                                                                            // true?

                                                                            hand_extra
                                                                                .pending_actions
                                                                                .push_back(action);
                                                                            game_msg_send.borrow().as_ref().unwrap().unbounded_send(ni_ty::protocol::GameMessageC2S::ApplyHandAction { action }.into()).unwrap();
                                                                        }

                                                                        true
                                                                    } else {
                                                                        log::debug!(
                                                                            "can't add {:?} to {:?}",
                                                                            back_card, target_stack
                                                                        );

                                                                        false
                                                                    }
                                                                } else {
                                                                    false
                                                                }
                                                            } else {
                                                                false
                                                            }
                                                        } else {
                                                            false
                                                        };

                                                        if success
                                                            || (!held.mouse_released
                                                                && settings.drag)
                                                        {
                                                            hand_extra.my_held_state = None;
                                                        } else if !mouse_pressed {
                                                            held.mouse_released = true;
                                                        }
                                                    }
                                                }
                                            }
                                        } else {
                                            if let Some(held_state) = &hand_extra.my_held_state {
                                                if !held_state.mouse_released {
                                                    hand_extra.my_held_state = None;
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
                                            .unbounded_send(
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

                            (Cow::Owned(pred_hand_state), my_location.inverted)
                        } else {
                            (Cow::Borrowed(real_hand_state), false)
                        };
                        let _ = real_hand_state;

                        mq::clear_background(BACKGROUND_COLOR);

                        for (idx, player_state) in pred_hand_state.players().iter().enumerate() {
                            let location = metrics.player_loc(idx);
                            let position =
                                mq::Vec2::from(location.pos()) + mq::Vec2::from(screen_center);

                            mq::set_camera(&normal_camera);
                            if let Some(player) = shared.game.players.get(&player_state.player_id())
                            {
                                let name_pos = if location.inverted == self_inverted {
                                    (
                                        position[0] + metrics.player_hand_width() / 2.0,
                                        position[1] - 20.0,
                                    )
                                } else {
                                    (
                                        screen_center.0
                                            - location.x
                                            - metrics.player_hand_width() / 2.0,
                                        screen_center.1 - metrics::PLAYER_Y + 20.0,
                                    )
                                };

                                if shared.game.master_player == player_state.player_id() {
                                    mq::draw_poly(
                                        name_pos.0,
                                        if location.inverted == self_inverted {
                                            name_pos.1 - 20.0
                                        } else {
                                            name_pos.1 + 20.0
                                        },
                                        4,
                                        10.0,
                                        0.0,
                                        mq::YELLOW,
                                    );
                                }
                                draw_text_centered(
                                    &player.name,
                                    name_pos.0,
                                    name_pos.1,
                                    40,
                                    mq::BLACK,
                                );
                            }

                            if location.inverted != self_inverted {
                                mq::set_camera(&inverted_camera);
                            }

                            let held_info = if Some(idx) == my_player_idx {
                                hand_extra.my_held_state.as_ref().map(|x| x.info)
                            } else {
                                hand_extra.mouse_states[idx]
                                    .as_ref()
                                    .and_then(|state| state.inner.held)
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
                                let nerts_stack_pos =
                                    mq::Vec2::from(metrics.player_stack_pos(
                                        ni_ty::PlayerStackLocation::Nerts,
                                        location,
                                    )) + mq::Vec2::from(screen_center);

                                for i in 0..(player_state.nerts_stack().len() - 1) {
                                    draw_back(
                                        nerts_stack_pos[0]
                                            + (i as f32) * metrics::HORIZONTAL_STACK_SPACING,
                                        nerts_stack_pos[1],
                                        player_state.player_id(),
                                    );
                                }
                                let card = player_state.nerts_stack().last().unwrap();

                                if started {
                                    if !matches!(
                                        held_info,
                                        Some(ni_ty::HeldInfo {
                                            src: ni_ty::PlayerStackLocation::Nerts,
                                            ..
                                        })
                                    ) {
                                        draw_card(
                                            card.card,
                                            nerts_stack_pos[0]
                                                + ((player_state.nerts_stack().len() - 1) as f32)
                                                    * metrics::HORIZONTAL_STACK_SPACING,
                                            nerts_stack_pos[1],
                                        );
                                    }
                                } else {
                                    draw_back(
                                        nerts_stack_pos[0]
                                            + ((player_state.nerts_stack().len() - 1) as f32)
                                                * metrics::HORIZONTAL_STACK_SPACING,
                                        nerts_stack_pos[1],
                                        player_state.player_id(),
                                    );
                                }
                            }

                            for (i, stack) in player_state.tableau_stacks().iter().enumerate() {
                                let cards = stack.cards();
                                let cards = if let Some(ni_ty::HeldInfo {
                                    src: ni_ty::PlayerStackLocation::Tableau(stack_idx),
                                    count,
                                    ..
                                }) = held_info
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

                                let loc = ni_ty::PlayerStackLocation::Tableau(i as u8);
                                let pos = mq::Vec2::from(metrics.player_stack_pos(loc, location))
                                    + mq::Vec2::from(screen_center);

                                if started {
                                    draw_vertical_stack_cards(cards, pos[0], pos[1]);
                                } else {
                                    draw_back(pos[0], pos[1], player_state.player_id());
                                }
                            }

                            let stock_pos = mq::Vec2::from(
                                metrics
                                    .player_stack_pos(ni_ty::PlayerStackLocation::Stock, location),
                            ) + mq::Vec2::from(screen_center);
                            if player_state.stock_stack().len() > 0 {
                                draw_back(stock_pos[0], stock_pos[1], player_state.player_id());
                            } else {
                                draw_placeholder(stock_pos[0], stock_pos[1]);
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
                            }) = held_info
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
                                let waste_pos =
                                    mq::Vec2::from(metrics.player_stack_pos(
                                        ni_ty::PlayerStackLocation::Waste,
                                        location,
                                    )) + mq::Vec2::from(screen_center);

                                draw_horizontal_stack_cards(
                                    waste_cards,
                                    waste_pos[0],
                                    waste_pos[1],
                                );
                            }

                            if hand_extra.stalled {
                                mq::draw_text(
                                    "Shuffling soon if game remains stalled...",
                                    stock_pos[0],
                                    stock_pos[1] + metrics::CARD_HEIGHT + 30.0,
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
                            let loc = ni_ty::StackLocation::Lake(i as u16);
                            let pos = mq::Vec2::from(metrics.stack_pos(loc))
                                + mq::Vec2::from(screen_center);

                            match stack.cards().last() {
                                None => {
                                    draw_placeholder(pos[0], pos[1]);
                                }
                                Some(card) => {
                                    draw_card(card.card, pos[0], pos[1]);
                                }
                            }
                        }

                        for (idx, value) in hand_extra.mouse_states.iter_mut().enumerate() {
                            if let Some(state) = value {
                                let location = metrics.player_loc(idx);

                                if location.inverted != self_inverted {
                                    mq::set_camera(&inverted_camera);
                                } else {
                                    mq::set_camera(&normal_camera);
                                }

                                state.step(mq::get_frame_time());
                                let mouse_pos = state.get_pos();

                                if let Some(held) = state.inner.held {
                                    let stack = pred_hand_state.players()[idx].stack_at(held.src);
                                    if let Some(stack) = stack {
                                        let cards = stack.cards();
                                        if (held.count as usize) <= cards.len() {
                                            let cards =
                                                &cards[(cards.len() - held.count as usize)..];

                                            if cards[0].card == held.top_card {
                                                draw_vertical_stack_cards(
                                                    cards,
                                                    screen_center.0 + mouse_pos[0] - held.offset.0,
                                                    screen_center.1 + mouse_pos[1] - held.offset.1,
                                                );
                                            }
                                        }
                                    }
                                }

                                mq::draw_texture_ex(
                                    &cursors_texture,
                                    screen_center.0 + mouse_pos[0] - 1.0,
                                    screen_center.1 + mouse_pos[1] - 1.0,
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
                                let stack = my_player_state.stack_at(held.info.src);
                                if let Some(stack) = stack {
                                    let stack_cards = stack.cards();
                                    if stack_cards.len() >= held.info.count as usize {
                                        let cards = &stack_cards
                                            [(stack_cards.len() - held.info.count as usize)..];

                                        draw_vertical_stack_cards(
                                            cards,
                                            mouse_pos[0] - held.info.offset.0,
                                            mouse_pos[1] - held.info.offset.1,
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

                        if !started {
                            mq::draw_rectangle(
                                0.0,
                                screen_center.1 - 70.0,
                                screen_size.0,
                                140.0,
                                NERTS_OVERLAY_COLOR,
                            );

                            if let Some(expected_start_time) = hand_extra.expected_start_time {
                                if let Some(time_until) = expected_start_time
                                    .checked_duration_since(web_time::Instant::now())
                                {
                                    draw_text_centered(
                                        &(time_until.as_secs() + 1).to_string(),
                                        screen_center.0,
                                        screen_center.1,
                                        100,
                                        NERTS_TEXT_COLOR,
                                    );
                                }
                            }
                        }

                        let mut next_state = State::GameHand {
                            my_player_idx,
                            show_settings,
                        };

                        egui_macroquad::ui(|egui_ctx| {
                            let ui_scale = scale / egui_ctx.zoom_factor();

                            egui::CentralPanel::default()
                                .frame(
                                    egui::Frame::none()
                                        .inner_margin(egui::style::Margin::same(SCREEN_MARGIN)),
                                )
                                .show(egui_ctx, |ui| {
                                    ui.set_enabled(interaction_enabled);

                                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Min), |ui| {
                                        if ui.button("Leave").clicked() {
                                            game_msg_send
                                                .borrow()
                                                .as_ref()
                                                .unwrap()
                                                .unbounded_send(ConnectionMessage::Leave)
                                                .unwrap();
                                        }

                                        if ui.button("Settings").clicked() {
                                            next_state = State::GameNeutral { show_settings: true };
                                        }
                                    });

                                    ui.label(format!(
                                        "Room Code: {}",
                                        to_full_game_id_str(shared.server_id, shared.game.id)
                                    ));

                                    if let Some(my_player_idx) = my_player_idx {
                                        let my_player_state = &pred_hand_state.players()[my_player_idx];

                                        if my_player_state.nerts_stack().len() < 1 {
                                            let location = metrics.player_loc(my_player_idx);
                                            let position = mq::Vec2::from(location.pos()) + mq::Vec2::from(screen_center);

                                            ui.allocate_ui_at_rect(
                                                egui::Rect {
                                                    min: egui::Pos2::new(position[0], position[1]),
                                                    max: egui::Pos2::new(
                                                        position[0] + 12.0 * metrics::HORIZONTAL_STACK_SPACING + metrics::CARD_WIDTH,
                                                        position[1] + metrics::CARD_HEIGHT,
                                                    )
                                                } * ui_scale,
                                                |ui| {
                                                    ui.centered_and_justified(|ui| {
                                                        if ui.button("Nerts!").clicked() {
                                                            hand_extra.self_called_nerts = true;
                                                            game_msg_send
                                                                .borrow()
                                                                .as_ref()
                                                                .unwrap()
                                                                .unbounded_send(
                                                                    ni_ty::protocol::GameMessageC2S::CallNerts.into(),
                                                                )
                                                                .unwrap();

                                                            let mut settings_lock = settings_mutex.lock().unwrap();
                                                            let settings = &mut *settings_lock;

                                                            if settings.nerts_callout {
                                                                macroquad::audio::play_sound_once(&nerts_callout);
                                                            }
                                                        }
                                                    });
                                                },
                                            );
                                        }
                                    }
                                });

                            if show_settings {
                                if !render_settings_window(&egui_ctx, &settings_mutex) {
                                    next_state = State::GameNeutral {
                                        show_settings: false,
                                    };
                                }
                            }
                        });

                        egui_macroquad::draw();

                        next_state
                    } else {
                        State::GameNeutral { show_settings }
                    }
                } else {
                    State::from_connection_state(&*lock)
                }
            }
            State::GameEnd { scores } => {
                mq::clear_background(BACKGROUND_COLOR);

                let mut lock = game_info_mutex.lock().unwrap();
                if let Some(shared) = (*lock).as_info_mut() {
                    match &shared.game.hand {
                        None => {
                            let mut go_next = false;

                            egui_macroquad::ui(|egui_ctx| {
                                egui::CentralPanel::default()
                                    .frame(egui::Frame::none())
                                    .show(egui_ctx, |ui| {
                                        let ui_screen_width =
                                            mq::screen_width() / egui_ctx.zoom_factor();
                                        let ui_screen_height =
                                            mq::screen_height() / egui_ctx.zoom_factor();

                                        let row_height = 25.0;

                                        let box_width = 250.0;
                                        let box_height = (row_height + ui.spacing().item_spacing.y)
                                            * (scores.len() as f32)
                                            + row_height;

                                        let box_x = ui_screen_width / 2.0 - box_width / 2.0;
                                        let box_y = ui_screen_height / 2.0 - box_height / 2.0;

                                        ui.allocate_ui_at_rect(
                                            egui::Rect {
                                                min: egui::Pos2::new(box_x, box_y),
                                                max: egui::Pos2::new(
                                                    box_x + box_width,
                                                    box_y + box_height,
                                                ),
                                            },
                                            |ui| {
                                                egui::Grid::new("end_scores")
                                                    .min_row_height(row_height)
                                                    .show(ui, |ui| {
                                                        for (player_id, score) in scores.iter() {
                                                            ui.label(score.to_string());

                                                            if let Some(player) =
                                                                shared.game.players.get(&player_id)
                                                            {
                                                                ui.label(&player.name);
                                                            }

                                                            ui.end_row();
                                                        }
                                                    });

                                                ui.vertical_centered(|ui| {
                                                    if ui.button("Continue").clicked() {
                                                        go_next = true;
                                                    }
                                                });
                                            },
                                        );
                                    });
                            });

                            egui_macroquad::draw();

                            if go_next {
                                State::GAME_NEUTRAL
                            } else {
                                State::GameEnd { scores }
                            }
                        }
                        Some(hand) => State::GameHand {
                            my_player_idx: hand
                                .players()
                                .iter()
                                .position(|player| player.player_id() == shared.my_player_id),
                            show_settings: false,
                        },
                    }
                } else {
                    State::from_connection_state(&*lock)
                }
            }
            State::LostConnection {
                was_connected,
                code,
            } => {
                mq::clear_background(BACKGROUND_COLOR);

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

                if let Some(details) = match code {
                    Some(ni_ty::protocol::CLOSE_KICK) => Some("Kicked by master"),
                    Some(ni_ty::protocol::CLOSE_TOO_NEW) => Some("Server is too old"),
                    Some(ni_ty::protocol::CLOSE_TOO_OLD) => {
                        Some("Your client is too old to connect to this server")
                    }
                    _ => None,
                } {
                    draw_text_centered(
                        details,
                        screen_center.0,
                        screen_center.1 + 60.0,
                        50,
                        mq::BLACK,
                    );
                }

                let mut go_back = false;

                egui_macroquad::ui(|egui_ctx| {
                    egui::CentralPanel::default()
                        .frame(egui::Frame::none())
                        .show(egui_ctx, |ui| {
                            ui.allocate_ui_at_rect(
                                egui::Rect {
                                    min: egui::Pos2::new(
                                        0.0,
                                        (screen_center.1 + 120.0) / egui_ctx.zoom_factor(),
                                    ),
                                    max: egui::Pos2::new(
                                        mq::screen_width() / egui_ctx.zoom_factor(),
                                        mq::screen_height() / egui_ctx.zoom_factor(),
                                    ),
                                },
                                |ui| {
                                    ui.vertical_centered(|ui| {
                                        if ui.button("Main Menu").clicked() {
                                            go_back = true;
                                        }
                                    });
                                },
                            );
                        });
                });

                egui_macroquad::draw();

                if go_back {
                    State::MAIN_MENU
                } else {
                    State::LostConnection {
                        was_connected,
                        code,
                    }
                }
            }
        };

        match events_recv.try_next() {
            Ok(Some(evt)) => match evt {
                ConnectionEvent::HandInit => {
                    let mut settings_lock = settings_mutex.lock().unwrap();
                    let settings = &mut *settings_lock;

                    if settings.round_start_music {
                        println!("playing sound");
                        macroquad::audio::play_sound_once(&round_start_music);
                    }
                }
                ConnectionEvent::PlayerHandAction(action) => {
                    let mut settings_lock = settings_mutex.lock().unwrap();
                    let settings = &mut *settings_lock;

                    if settings.suit_callouts {
                        match action {
                            ni_ty::HandAction::Move { to, .. } => {
                                if matches!(to, ni_ty::StackLocation::Lake(_)) {
                                    let mut lock = game_info_mutex.lock().unwrap();
                                    if let Some(shared) = (*lock).as_info_mut() {
                                        if let Some(hand) = &shared.game.hand {
                                            if let Some(stack) = hand.stack_at(to) {
                                                if let Some(top) = stack.last() {
                                                    if top.card.rank == ni_ty::Rank::ACE {
                                                        macroquad::audio::play_sound_once(
                                                            match top.card.suit {
                                                                ni_ty::Suit::Spades => {
                                                                    &suit_callout_spades
                                                                }
                                                                ni_ty::Suit::Diamonds => {
                                                                    &suit_callout_diamonds
                                                                }
                                                                ni_ty::Suit::Clubs => {
                                                                    &suit_callout_clubs
                                                                }
                                                                ni_ty::Suit::Hearts => {
                                                                    &suit_callout_hearts
                                                                }
                                                            },
                                                        );
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }
                ConnectionEvent::NertsCalled => {
                    let mut settings_lock = settings_mutex.lock().unwrap();
                    let settings = &mut *settings_lock;

                    if settings.nerts_callout {
                        macroquad::audio::play_sound_once(&nerts_callout);
                    }
                }
            },
            Ok(None) => unreachable!(),
            Err(_) => {
                // no events
            }
        }

        mq::next_frame().await
    }
}
