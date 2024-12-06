#![allow(clippy::collapsible_if)]
#![allow(clippy::collapsible_else_if)]

use macroquad::logging as log;
use macroquad::prelude as mq;
use nertsio_types as ni_ty;
use std::cell::RefCell;
use std::collections::VecDeque;
#[cfg(target_family = "wasm")]
use std::future::Future;
use std::sync::Arc;

mod connection;
mod settings;
mod util;
mod views;

mod licenses {
    include!(concat!(env!("OUT_DIR"), "/generated.rs"));
}

use connection::{ConnectionEvent, ConnectionMessage};
use settings::Settings;

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

const MAX_INTERPOLATION_TIME: f32 = 0.3;

pub enum ConnectionState {
    NotConnected {
        expected: bool,
        error: Option<String>,
    },
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

pub struct SharedInfo {
    game: ni_ty::GameState,
    my_player_id: u8,
    server_id: u8,
    region: Option<ni_ty::RegionInfo<'static>>,
    hand_extra: Option<HandExtra>,
    new_end_scores: Option<Vec<(u8, i32)>>,
    ping: Option<std::time::Duration>,
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
    is_drag: bool,
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

fn get_window_conf() -> mq::Conf {
    mq::Conf {
        window_title: "nertsio".to_owned(),
        window_width: 1600,
        window_height: 1000,
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

pub enum LoadState<T> {
    Pending(futures_channel::oneshot::Receiver<Result<T, anyhow::Error>>),
    Done(Result<T, anyhow::Error>),
}

impl<T> LoadState<T> {
    pub fn tick(self) -> Self {
        match self {
            LoadState::Pending(mut channel) => match channel.try_recv() {
                Ok(None) => LoadState::Pending(channel),
                Ok(Some(value)) => LoadState::Done(value),
                Err(futures_channel::oneshot::Canceled) => {
                    LoadState::Done(Err(anyhow::anyhow!("Canceled")))
                }
            },
            LoadState::Done(value) => LoadState::Done(value),
        }
    }

    pub fn is_done(&self) -> bool {
        match self {
            LoadState::Done(_) => true,
            _ => false,
        }
    }
}

impl<T> From<futures_channel::oneshot::Receiver<Result<T, anyhow::Error>>> for LoadState<T> {
    fn from(src: futures_channel::oneshot::Receiver<Result<T, anyhow::Error>>) -> Self {
        LoadState::Pending(src)
    }
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

    let mut coordinator_url = "https://coordinator.nerts.io/".to_owned();

    {
        let mut args = std::env::args();
        let _ = args.next();

        if let Some(arg) = args.next() {
            if arg == "--coordinator-url" {
                coordinator_url = args.next().expect("Missing value for coordinator-url");
                if !coordinator_url.ends_with('/') {
                    coordinator_url.push('/');
                }
            } else {
                panic!("unknown argument");
            }
        }

        if args.next().is_some() {
            panic!("unknown argument");
        }
    }

    #[cfg(not(target_family = "wasm"))]
    let async_rt = tokio::runtime::Runtime::new().unwrap();

    #[cfg(target_family = "wasm")]
    let async_rt = WasmAsyncRt;

    let font = mq::load_ttf_font_from_bytes(
        &egui::FontDefinitions::default()
            .font_data
            .get("Ubuntu-Light")
            .unwrap()
            .font,
    )
    .unwrap();

    let cards_texture_standard =
        mq::Texture2D::from_file_with_format(nertsio_textures::CARDS, Some(mq::ImageFormat::Png));
    let cards_texture_hivis = mq::Texture2D::from_file_with_format(
        nertsio_textures::CARDS_HIVIS,
        Some(mq::ImageFormat::Png),
    );
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

    let game_info_mutex = Arc::new(std::sync::Mutex::new(ConnectionState::NotConnected {
        expected: true,
        error: None,
    }));
    let game_msg_send = RefCell::new(None);

    let (events_send, mut events_recv) = futures_channel::mpsc::unbounded();

    let http_client = reqwest::Client::new();

    let settings_mutex = settings::init_settings(async_rt.handle());

    let get_cards_texture = || {
        let settings = settings_mutex.lock().unwrap();
        match settings.card_theme {
            settings::CardTheme::Standard => &cards_texture_standard,
            settings::CardTheme::HighVisibility => &cards_texture_hivis,
        }
    };

    let mut ctx = views::GameContext {
        async_rt: async_rt.handle().clone(),
        game_info_mutex: game_info_mutex.clone(),
        http_client,
        coordinator_url: &coordinator_url,
        settings_mutex: settings_mutex.clone(),
        events_send,
        game_msg_send,
        quit: false,

        cards_texture: get_cards_texture(),
        backs_texture,
        cursors_texture,
        placeholder_texture,
        font,
        nerts_callout: &nerts_callout,
    };

    let mut view: views::View = views::MainMenuView::init(&ctx).into();

    egui_macroquad::cfg(|egui_ctx| {
        let mut visuals = egui::Visuals::light();

        visuals.widgets.noninteractive.fg_stroke.color = egui::Color32::from_gray(40);

        egui_ctx.set_visuals(visuals);
    });

    while !ctx.quit {
        let ui_scale = (mq::screen_width() / 1.5).min(mq::screen_height()) / 1080.0 * 3.0;

        egui_macroquad::cfg(|egui_ctx| {
            egui_ctx.set_zoom_factor(ui_scale);
        });

        mq::set_default_camera();

        ctx.cards_texture = get_cards_texture();

        view = views::ViewImpl::tick(view, &mut ctx);

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
                        if let ni_ty::HandAction::Move { to, .. } = action {
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
