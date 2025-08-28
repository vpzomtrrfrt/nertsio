use crate::settings::DragMode;
use crate::{ConnectionEvent, ConnectionMessage, ConnectionState, Settings};
use macroquad::hash;
use macroquad::logging as log;
use macroquad::miniquad;
use macroquad::prelude as mq;
use nertsio_common as common;
use nertsio_types as ni_ty;
use nertsio_ui_metrics as metrics;
use std::cell::RefCell;
use std::sync::{Arc, Mutex};

mod ingame_hand;

use ingame_hand::IngameHandView;

const BACKGROUND_COLOR: mq::Color = mq::Color::new(0.1, 0.6, 0.1, 1.0);
const BASE_SCREEN_MARGIN: f32 = 5.0;
const CARD_SIZE: mq::Vec2 = mq::Vec2 {
    x: metrics::CARD_WIDTH,
    y: metrics::CARD_HEIGHT,
};

const CAN_QUIT: bool = cfg!(not(any(
    target_family = "wasm",
    target_os = "android",
    target_os = "ios"
)));

const PUBLIC_GAMES_REFRESH_DELAY: std::time::Duration = std::time::Duration::from_secs(5);

#[allow(clippy::enum_variant_names)]
#[enum_dispatch::enum_dispatch]
pub enum View {
    CreditsView,
    MainMenuView,
    JoinGameFormView,
    ConnectingView,
    IngameNeutralView,
    IngameHandView,
    IngameEndView,
    LostConnectionView,
}

#[enum_dispatch::enum_dispatch(View)]
pub trait ViewImpl {
    fn tick(self, ctx: &mut GameContext) -> View;

    fn should_clear_last_menu_mouse_position(&self) -> bool {
        true
    }
}

impl View {
    pub fn from_connection_state(src: &ConnectionState, ctx: &GameContext) -> Self {
        match src {
            ConnectionState::NotConnected {
                expected: true,
                error: _,
            } => MainMenuView::init(ctx).into(),
            ConnectionState::NotConnected {
                expected: false,
                error,
            } => LostConnectionView {
                was_connected: true,
                error: error.clone(),
            }
            .into(),
            _ => LostConnectionView {
                was_connected: true,
                error: None,
            }
            .into(),
        }
    }
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

pub struct GameContext<'a> {
    pub async_rt: crate::AsyncRt,
    pub game_info_mutex: Arc<Mutex<ConnectionState>>,
    pub http_client: reqwest::Client,
    pub coordinator_url: &'a str,
    pub settings_mutex: Arc<Mutex<Settings>>,
    pub events_send: futures_channel::mpsc::UnboundedSender<ConnectionEvent>,
    pub game_msg_send: RefCell<Option<futures_channel::mpsc::UnboundedSender<ConnectionMessage>>>,
    pub quit: bool,
    pub regions_list_state: crate::LoadState<Vec<ni_ty::RegionInfo<'static>>>,

    pub cards_texture: &'a mq::Texture2D,
    pub backs_texture: mq::Texture2D,
    pub cursors_texture: mq::Texture2D,
    pub placeholder_texture: mq::Texture2D,
    pub font: mq::Font,
    pub nerts_callout: &'a macroquad::audio::Sound,
    pub flip_sound: &'a macroquad::audio::Sound,
    pub gather_sound: &'a macroquad::audio::Sound,
    pub pickup_sound: &'a macroquad::audio::Sound,
    pub place_sound: &'a macroquad::audio::Sound,
    pub shuffle_sound: &'a macroquad::audio::Sound,
}

impl<'a> GameContext<'a> {
    fn do_connection(&self, connection_type: crate::connection::ConnectionType) {
        let (new_game_msg_send, game_msg_recv) = futures_channel::mpsc::unbounded();
        *self.game_msg_send.borrow_mut() = Some(new_game_msg_send);

        let handle = self.async_rt.clone();

        self.async_rt.spawn({
            let game_info_mutex = self.game_info_mutex.clone();
            let http_client = self.http_client.clone();
            let settings_mutex = self.settings_mutex.clone();
            let events_send = self.events_send.clone();

            let coordinator_url = self.coordinator_url.to_owned();

            async move {
                {
                    (*game_info_mutex.lock().unwrap()) = ConnectionState::Connecting;
                }
                let res = crate::connection::handle_connection(
                    &http_client,
                    &coordinator_url,
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
                        error: Some(err.to_string()),
                    };
                    log::error!("Failed to handle connection: {:?}", err);
                } else {
                    (*lock) = ConnectionState::NotConnected {
                        expected: true,
                        error: None,
                    };
                }
            }
        });
    }

    fn start_loading<T: Send + 'static>(
        &self,
        fut: impl crate::LoadFut<T>,
    ) -> crate::LoadChannel<T> {
        crate::start_loading(&self.async_rt, fut)
    }

    fn start_loading_public_games(
        &self,
    ) -> crate::LoadChannel<Vec<ni_ty::protocol::PublicGameInfoExpanded<'static>>> {
        let req_fut = self
            .http_client
            .get(format!("{}public_games", self.coordinator_url))
            .send();

        self.start_loading(async move {
            let resp = req_fut.await?.error_for_status()?;

            let resp: ni_ty::protocol::RespList<ni_ty::protocol::PublicGameInfoExpanded> =
                resp.json().await?;

            Result::<_, anyhow::Error>::Ok(resp.items)
        })
    }

    pub fn quit(&mut self) {
        self.quit = true;
    }

    fn draw_text(&self, text: &str, x: f32, y: f32, font_size: u16, color: mq::Color) {
        mq::draw_text_ex(
            text,
            x,
            y,
            mq::TextParams {
                font_size,
                color,
                font: Some(&self.font),
                ..Default::default()
            },
        );
    }

    fn draw_text_centered(&self, text: &str, x: f32, y: f32, font_size: u16, color: mq::Color) {
        let metrics = mq::measure_text(
            text,
            Some(&self.font),
            font_size,
            mq::camera_font_scale(font_size.into()).1,
        );

        self.draw_text(
            text,
            x - metrics.width / 2.0,
            y - metrics.height / 2.0 + metrics.offset_y,
            font_size,
            color,
        );
    }

    fn draw_vertical_stack_cards(&self, cards: &[ni_ty::CardInstance], x: f32, y: f32) {
        if cards.is_empty() {
            self.draw_placeholder(x, y);
        } else {
            for (i, card) in cards.iter().enumerate() {
                self.draw_card(
                    card.card,
                    x,
                    y + (i as f32) * metrics::VERTICAL_STACK_SPACING,
                );
            }
        }
    }

    fn draw_card(&self, card: ni_ty::Card, x: f32, y: f32) {
        mq::draw_texture_ex(
            self.cards_texture,
            x,
            y,
            mq::WHITE,
            mq::DrawTextureParams {
                source: Some(get_card_rect(card)),
                dest_size: Some(CARD_SIZE),
                ..Default::default()
            },
        );
    }

    fn draw_placeholder(&self, x: f32, y: f32) {
        mq::draw_texture_ex(
            &self.placeholder_texture,
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
                dest_size: Some(CARD_SIZE),
                ..Default::default()
            },
        );
    }

    fn draw_horizontal_stack_cards(&self, cards: &[ni_ty::CardInstance], x: f32, y: f32) {
        if cards.is_empty() {
            mq::draw_texture_ex(
                &self.placeholder_texture,
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
                    dest_size: Some(CARD_SIZE),
                    ..Default::default()
                },
            );
        } else {
            for (i, card) in cards.iter().enumerate() {
                self.draw_card(
                    card.card,
                    x + (i as f32) * metrics::HORIZONTAL_STACK_SPACING,
                    y,
                );
            }
        }
    }

    fn draw_back(&self, x: f32, y: f32, owner_id: u8) {
        let bg_color = crate::PLAYER_COLORS[(owner_id >> 4) as usize];
        let fg_color = crate::PLAYER_COLORS[(owner_id & 0xF) as usize];

        mq::draw_texture_ex(
            &self.backs_texture,
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
                dest_size: Some(CARD_SIZE),
                ..Default::default()
            },
        );

        mq::draw_texture_ex(
            &self.backs_texture,
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
                dest_size: Some(CARD_SIZE),
                ..Default::default()
            },
        );
    }

    fn draw_cursor(&self, x: f32, y: f32, player_id: u8) {
        mq::draw_texture_ex(
            &self.cursors_texture,
            x,
            y,
            crate::PLAYER_COLORS[(player_id >> 4) as usize],
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

    pub fn play_sound_for_action(&self, action: ni_ty::HandAction) {
        #[allow(unreachable_patterns)]
        let sound = match action {
            ni_ty::HandAction::ReturnStock => Some(&self.gather_sound),
            ni_ty::HandAction::FlipStock => Some(&self.flip_sound),
            ni_ty::HandAction::Move { .. } => Some(&self.place_sound),
            ni_ty::HandAction::ShuffleStock { .. } => Some(&self.shuffle_sound),
            _ => None,
        };

        if let Some(sound) = sound {
            macroquad::audio::play_sound_once(sound);
        }
    }
}

pub fn render_settings_window(egui_ctx: &egui::Context, ctx: &GameContext) -> bool {
    let menu_width = 300.0;

    let mut open = true;

    egui::containers::Window::new("Settings")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .open(&mut open)
        .show(egui_ctx, |ui| {
            let mut settings_lock = ctx.settings_mutex.lock().unwrap();
            let settings = &mut *settings_lock;

            ui.vertical_centered(|ui| {
                ui.allocate_ui_with_layout(
                    egui::Vec2::new(menu_width, 0.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.label("Movement Mode");
                        ui.indent(hash!(), |ui| {
                            ui.radio_value(&mut settings.drag_mode, DragMode::Click, "Pickup");

                            ui.radio_value(
                                &mut settings.drag_mode,
                                DragMode::Drag,
                                "Drag-and-Drop",
                            );

                            ui.radio_value(&mut settings.drag_mode, DragMode::Hybrid, "Hybrid");
                        });

                        ui.label("Card Theme");
                        ui.indent(hash!(), |ui| {
                            ui.radio_value(
                                &mut settings.card_theme,
                                crate::settings::CardTheme::Standard,
                                "Standard",
                            );
                            ui.radio_value(
                                &mut settings.card_theme,
                                crate::settings::CardTheme::HighVisibility,
                                "High Visibility",
                            );
                        });

                        ui.label("Sound");
                        ui.indent(hash!(), |ui| {
                            ui.checkbox(&mut settings.music, "Round Start Music");
                            ui.checkbox(&mut settings.sounds, "Sounds");
                        });

                        ui.label("Preferred Region");
                        ui.indent(hash!(), |ui| match &ctx.regions_list_state {
                            crate::LoadState::Pending(_) => {
                                ui.label("Loading...");
                            }
                            crate::LoadState::Done(Err(err)) => {
                                ui.label(format!("Failed to load: {:?}", err));
                            }
                            crate::LoadState::Done(Ok(list)) => {
                                ui.radio_value(&mut settings.preferred_region, None, "Automatic");

                                for region in list {
                                    let selected = settings.preferred_region.as_deref()
                                        == Some(region.id.as_ref());
                                    let mut res = ui.radio(selected, &region.name[..]);
                                    if res.clicked() {
                                        if !selected {
                                            settings.preferred_region =
                                                Some(region.id.as_ref().to_owned());
                                            res.mark_changed();
                                        }
                                    }
                                }
                            }
                        });
                    },
                );
            });
        });

    open
}

pub struct MainMenuView {
    show_settings: bool,

    public_games_state: crate::LoadState<Vec<ni_ty::protocol::PublicGameInfoExpanded<'static>>>,
    public_games_done_at: Option<web_time::Instant>,
    public_games_reload_channel:
        Option<crate::LoadChannel<Vec<ni_ty::protocol::PublicGameInfoExpanded<'static>>>>,
}

impl MainMenuView {
    pub fn init(ctx: &GameContext) -> MainMenuView {
        Self {
            show_settings: false,
            public_games_state: ctx.start_loading_public_games().into(),
            public_games_done_at: None,
            public_games_reload_channel: None,
        }
    }
}

impl ViewImpl for MainMenuView {
    fn tick(mut self, ctx: &mut GameContext) -> View {
        if self.public_games_state.is_done() {
            match self.public_games_reload_channel.take() {
                Some(channel) => {
                    let new_state = crate::LoadState::from(channel).tick();
                    match new_state {
                        crate::LoadState::Pending(channel) => {
                            self.public_games_reload_channel = Some(channel);
                        }
                        crate::LoadState::Done(value) => {
                            self.public_games_state = crate::LoadState::Done(value);
                            self.public_games_done_at = Some(web_time::Instant::now());
                        }
                    }
                }
                None => {
                    let done_at = self
                        .public_games_done_at
                        .expect("Missing public_games_done_at, but it is done");
                    if web_time::Instant::now().duration_since(done_at)
                        >= PUBLIC_GAMES_REFRESH_DELAY
                    {
                        self.public_games_reload_channel = Some(ctx.start_loading_public_games());
                    }
                }
            }
        } else {
            self.public_games_state = self.public_games_state.tick();

            if self.public_games_state.is_done() {
                self.public_games_done_at = Some(web_time::Instant::now());
            }
        }

        mq::clear_background(BACKGROUND_COLOR);

        let button_height = 20.0;

        let button_count = 6;

        let menu_width = 150.0;

        let mut new_state: Option<View> = None;

        egui_macroquad::ui(|egui_ctx| {
            let screen_margin = get_screen_margin(egui_ctx);

            egui::SidePanel::left("main_menu")
                .frame(egui::Frame::none().inner_margin(egui::Margin {
                    left: screen_margin.left,
                    right: BASE_SCREEN_MARGIN,
                    top: screen_margin.top,
                    bottom: screen_margin.bottom,
                }))
                .show(egui_ctx, |ui| {
                    if self.show_settings {
                        ui.disable();
                    }

                    let menu_height = button_height * (button_count as f32)
                        + ((button_count - 1) as f32) * ui.spacing().item_spacing.y
                        + 60.0;

                    let menu_y = ui.max_rect().height() / 2.0 - menu_height / 2.0;

                    ui.allocate_space(egui::Vec2 { x: 0.0, y: menu_y });

                    ui.allocate_ui(
                        egui::Vec2 {
                            x: menu_width,
                            y: menu_height,
                        },
                        |ui| {
                            ui.with_layout(egui::Layout::top_down(egui::Align::Center), |ui| {
                                ui.label(egui::RichText::new("nertsio").size(40.0));
                            });

                            let menu_button = |ui: &mut egui::Ui, label| {
                                ui.add(
                                    egui::widgets::Button::new(label)
                                        .min_size(egui::Vec2::new(menu_width, button_height)),
                                )
                                .clicked()
                            };

                            {
                                let mut settings_lock = ctx.settings_mutex.lock().unwrap();
                                let settings = &mut *settings_lock;

                                ui.horizontal(|ui| {
                                    ui.label("Name:");
                                    handle_input_response(
                                        ui.add(
                                            egui::widgets::TextEdit::singleline(&mut settings.name)
                                                .char_limit(common::MAX_NAME_LENGTH),
                                        ),
                                    );
                                });
                            }

                            if menu_button(ui, "Create New Game") {
                                ctx.do_connection(crate::connection::ConnectionType::CreateGame {});
                                new_state = Some(ConnectingView.into());
                            } else if menu_button(ui, "Join Private Game") {
                                new_state = Some(JoinGameFormView::default().into());
                            } else if menu_button(ui, "Settings") {
                                self.show_settings = true;
                            } else if menu_button(ui, "Credits") {
                                new_state = Some(CreditsView.into());
                            } else if CAN_QUIT && menu_button(ui, "Quit") {
                                ctx.quit();
                            }
                        },
                    );
                });

            egui::CentralPanel::default()
                .frame(egui::Frame::none().inner_margin(egui::Margin {
                    left: BASE_SCREEN_MARGIN,
                    right: screen_margin.right,
                    top: screen_margin.top,
                    bottom: screen_margin.bottom,
                }))
                .show(egui_ctx, |ui| {
                    ui.heading("Public Games");

                    match &self.public_games_state {
                        crate::LoadState::Pending(_) => {
                            ui.label("Loading...");
                        }
                        crate::LoadState::Done(Err(err)) => {
                            ui.label(format!("Failed to load: {:?}", err));
                        }
                        crate::LoadState::Done(Ok(list)) => {
                            egui::Grid::new("public_game_list").show(ui, |ui| {
                                if list.is_empty() {
                                    ui.label("No games found.");
                                } else {
                                    for game in list {
                                        ui.label(crate::util::to_full_game_id_str(
                                            game.server.server_id,
                                            game.game_id,
                                        ));
                                        ui.label(format!("{} players", game.players));
                                        ui.label(if game.waiting { "waiting" } else { "playing" });
                                        if ui.button("Join").clicked() {
                                            ctx.do_connection(
                                                crate::connection::ConnectionType::JoinPublicGame {
                                                    server: game.server.clone(),
                                                    game_id: game.game_id,
                                                },
                                            );
                                            new_state = Some(ConnectingView.into());
                                        }

                                        ui.end_row();
                                    }
                                }
                            });
                        }
                    }
                });

            if self.show_settings {
                if !render_settings_window(egui_ctx, &ctx) {
                    self.show_settings = false;
                }
            }
        });

        egui_macroquad::draw();

        new_state.unwrap_or(self.into())
    }
}

pub struct JoinGameFormView {
    input: String,
    initing: bool,
}

impl Default for JoinGameFormView {
    fn default() -> Self {
        Self {
            input: String::new(),
            initing: true,
        }
    }
}

impl ViewImpl for JoinGameFormView {
    fn tick(mut self, ctx: &mut GameContext) -> View {
        let menu_width = 150.0;

        let button_height = 20.0;
        let button_count = 2;

        mq::clear_background(BACKGROUND_COLOR);

        let mut go_connect = false;
        let mut go_back = false;

        egui_macroquad::ui(|egui_ctx| {
            let screen_margin = get_screen_margin(egui_ctx);

            egui::CentralPanel::default()
                .frame(egui::Frame::none().inner_margin(screen_margin))
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
                                let response = ui.text_edit_singleline(&mut self.input);

                                if self.initing {
                                    response.request_focus();
                                }

                                if response.lost_focus()
                                    && ui.input(|i| i.key_pressed(egui::Key::Enter))
                                {
                                    go_connect = true;
                                }

                                handle_input_response(response);
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

        self.initing = false;

        if go_connect {
            if let Ok((server_id, game_id)) = crate::util::parse_full_game_id_str(&self.input) {
                ctx.do_connection(crate::connection::ConnectionType::JoinPrivateGame {
                    server_id,
                    game_id,
                });
                ConnectingView.into()
            } else {
                self.into()
            }
        } else if go_back {
            MainMenuView::init(ctx).into()
        } else {
            self.into()
        }
    }
}

#[derive(Default)]
pub struct ConnectingView;

impl ViewImpl for ConnectingView {
    fn tick(self, ctx: &mut GameContext) -> View {
        mq::clear_background(BACKGROUND_COLOR);

        if mq::is_key_pressed(mq::KeyCode::Escape) {
            ctx.game_msg_send
                .borrow()
                .as_ref()
                .unwrap()
                .unbounded_send(ConnectionMessage::Leave)
                .unwrap();
        }

        match *ctx.game_info_mutex.lock().unwrap() {
            ConnectionState::Connecting => self.into(),
            ConnectionState::Connected(_) => IngameNeutralView::default().into(),
            ConnectionState::NotConnected {
                expected,
                ref error,
            } => {
                if expected {
                    MainMenuView::init(ctx).into()
                } else {
                    LostConnectionView {
                        was_connected: false,
                        error: error.clone(),
                    }
                    .into()
                }
            }
        }
    }
}

#[derive(Default)]
pub struct IngameNeutralView {
    show_settings: bool,
    editing_game_settings: Option<ni_ty::GameSettings>,
}

impl ViewImpl for IngameNeutralView {
    fn tick(mut self, ctx: &mut GameContext) -> View {
        let mut ui_scale = None;
        egui_macroquad::cfg(|egui_ctx| {
            ui_scale = Some(egui_ctx.zoom_factor());
        });
        let ui_scale = ui_scale.unwrap();

        let mut lock = ctx.game_info_mutex.lock().unwrap();

        if !self.should_clear_last_menu_mouse_position() {
            if let Some(shared) = (*lock).as_info_mut() {
                let mouse_pos = mq::mouse_position();
                shared.my_last_menu_mouse_position =
                    Some((mouse_pos.0 / ui_scale, mouse_pos.1 / ui_scale));
            }
        }

        mq::clear_background(BACKGROUND_COLOR);

        if let Some(shared) = (*lock).as_info_mut() {
            match &shared.game.hand {
                None => {
                    if let Some(scores) = shared.new_end_scores.take() {
                        IngameEndView { scores }.into()
                    } else {
                        let can_add_player = shared
                            .game
                            .players
                            .values()
                            .filter(|x| !x.spectating)
                            .count()
                            < shared.game.settings.max_players.into();

                        let sorted = {
                            let mut result: Vec<u8> = shared.game.players.keys().copied().collect();
                            result.sort_by_key(|key| -shared.game.players[key].score);
                            result
                        };

                        let mut ui_scale = None;
                        egui_macroquad::ui(|egui_ctx| {
                            let screen_margin = get_screen_margin(egui_ctx);

                            ui_scale = Some(egui_ctx.zoom_factor());

                            egui::SidePanel::right("game_settings_panel")
                                .frame(egui::Frame::none().inner_margin(egui::Margin {
                                    left: BASE_SCREEN_MARGIN,
                                    right: screen_margin.right,
                                    top: screen_margin.top,
                                    bottom: screen_margin.bottom,
                                }))
                                .resizable(false)
                                .show(egui_ctx, |ui| {
                                    if self.show_settings || self.editing_game_settings.is_some() {
                                        ui.disable();
                                    }

                                    ui.with_layout(
                                        egui::Layout::top_down(egui::Align::Min),
                                        |ui| {
                                            ui.horizontal(|ui| {
                                                ui.heading("Game Settings");

                                                if shared.game.master_player == shared.my_player_id
                                                {
                                                    if ui.button("Edit").clicked() {
                                                        self.editing_game_settings =
                                                            Some(shared.game.settings.clone());
                                                    }
                                                }
                                            });

                                            ui.label(if shared.game.settings.public {
                                                egui::RichText::new("Public")
                                            } else {
                                                egui::RichText::new("Private")
                                                    .color(egui::Color32::YELLOW)
                                            });

                                            ui.label(format!(
                                                "Max Players: {}",
                                                shared.game.settings.max_players
                                            ));

                                            ui.label(format!(
                                                "Nerts Card Penalty: {}",
                                                shared.game.settings.nerts_card_penalty
                                            ));

                                            ui.label(format!(
                                                "Bot Difficulty: {}%",
                                                (shared.game.settings.bot_difficulty * 100.0)
                                                    .round()
                                            ));
                                        },
                                    );

                                    ui.with_layout(
                                        egui::Layout::top_down(egui::Align::Min),
                                        |ui| {
                                            ui.heading("Server Info");

                                            if let Some(region) = &shared.region {
                                                ui.label(format!("Region: {}", region.name));
                                            }

                                            if let Some(ping) = &shared.ping {
                                                ui.label(format!("Ping: {}ms", ping.as_millis()));
                                            }
                                        },
                                    );
                                });

                            egui::CentralPanel::default()
                                .frame(egui::Frame::none().inner_margin(egui::Margin {
                                    left: screen_margin.left,
                                    right: BASE_SCREEN_MARGIN,
                                    top: screen_margin.top,
                                    bottom: screen_margin.bottom,
                                }))
                                .show(egui_ctx, |ui| {
                                    if self.show_settings || self.editing_game_settings.is_some() {
                                        ui.disable();
                                    }


                                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Min), |ui| {
                                        if ui.button("Leave").clicked() {
                                            ctx.game_msg_send
                                                .borrow()
                                                .as_ref()
                                                .unwrap()
                                                .unbounded_send(crate::ConnectionMessage::Leave)
                                                .unwrap();
                                        }

                                        if ui.button("Settings").clicked() {
                                            self.show_settings = true;
                                        }
                                    });

                                    ui.label(format!("Room Code: {}", crate::util::to_full_game_id_str(shared.server_id, shared.game.id)));

                                    egui::Grid::new("scoreboard_grid").show(ui, |ui| {
                                        for key in sorted.iter() {
                                            let player = shared.game.players.get_mut(key).unwrap();

                                            ui.label(player.score.to_string());

                                            if *key == shared.my_player_id {
                                                if player.spectating {
                                                    if ui.add_enabled(can_add_player, egui::Button::new("Stop Spectating")).clicked() {
                                                        player.spectating = false;

                                                        ctx.game_msg_send
                                                            .borrow()
                                                            .as_ref()
                                                            .unwrap()
                                                            .unbounded_send(
                                                                ni_ty::protocol::GameMessageC2S::UpdateSelfSpectating {
                                                                    value: false,
                                                                }
                                                                .into(),
                                                            )
                                                            .unwrap();
                                                    }
                                                } else {
                                                    ui.horizontal(|ui| {
                                                        if ui.button(if player.ready { "Unready" } else { "Ready" }).clicked() {
                                                            let new_value = !player.ready;
                                                            player.ready = new_value;

                                                            ctx.game_msg_send
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

                                                        if ui.button("Spectate").clicked() {
                                                            player.spectating = true;

                                                            ctx.game_msg_send
                                                                .borrow()
                                                                .as_ref()
                                                                .unwrap()
                                                                .unbounded_send(
                                                                    ni_ty::protocol::GameMessageC2S::UpdateSelfSpectating {
                                                                        value: true,
                                                                    }
                                                                    .into(),
                                                                )
                                                                .unwrap();
                                                        }
                                                    });
                                                }
                                            } else {
                                                ui.label(if player.spectating { "Spectating" } else if player.ready { "Ready" } else { "Not Ready" });
                                            }

                                            ui.horizontal(|ui| {
                                                if shared.game.master_player == *key {
                                                    ui.colored_label(egui::Color32::YELLOW, "★");
                                                } else if shared.game.master_player == shared.my_player_id {
                                                    if ui.button("x").clicked() {
                                                        ctx.game_msg_send
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
                                        if ui.add_enabled(can_add_player, egui::Button::new("Add Bot")).clicked() {
                                            ctx.game_msg_send
                                                .borrow()
                                                .as_ref()
                                                .unwrap()
                                                .unbounded_send(
                                                    ni_ty::protocol::GameMessageC2S::AddBot.into(),
                                                )
                                                .unwrap();
                                        }

                                        let my_player_state = shared.game.players.get(&shared.my_player_id).unwrap();

                                        if my_player_state.ready || my_player_state.spectating {
                                            if ui.button("Force Start").clicked() {
                                                ctx.game_msg_send
                                                    .borrow()
                                                    .as_ref()
                                                    .unwrap()
                                                    .unbounded_send(
                                                        ni_ty::protocol::GameMessageC2S::ForceStart.into(),
                                                    )
                                                    .unwrap();
                                            }
                                        }
                                    }

                                });

                            if let Some(ref mut new_settings) = self.editing_game_settings {
                                let menu_width = 300.0;

                                let mut close = false;

                                egui::containers::Window::new("Edit Game Settings")
                                    .collapsible(false)
                                    .resizable(false)
                                    .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                                    .show(egui_ctx, |ui| {
                                        ui.vertical_centered(|ui| {
                                            ui.allocate_ui_with_layout(
                                                egui::Vec2::new(menu_width, 0.0),
                                                egui::Layout::top_down(egui::Align::Min),
                                                |ui| {
                                                    // the server is fine with reverting to private
                                                    // but it's probably misleading to allow it
                                                    ui.add_enabled(!shared.game.settings.public, egui::Checkbox::new(&mut new_settings.public, "Public"));

                                                    ui.label("Max Players");
                                                    ui.indent(hash!(), |ui| {
                                                        ui.add(egui::Slider::new(&mut new_settings.max_players, 1..=12));
                                                    });

                                                    ui.label("Nerts Card Penalty");
                                                    ui.indent(hash!(), |ui| {
                                                        for value in 0..=3 {
                                                            ui.radio_value(&mut new_settings.nerts_card_penalty, value, value.to_string());
                                                        }
                                                    });

                                                    let mut bot_difficulty_pct = new_settings.bot_difficulty * 100.0;

                                                    ui.label("Bot Difficulty");
                                                    ui.indent(hash!(), |ui| {
                                                        ui.add(egui::widgets::Slider::new(
                                                            &mut bot_difficulty_pct,
                                                            0.0..=100.0,
                                                        ).max_decimals(0));
                                                    });

                                                    new_settings.bot_difficulty = bot_difficulty_pct / 100.0;

                                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::BOTTOM), |ui| {
                                                        if ui.button("Save").clicked() {
                                                            ctx.game_msg_send.borrow().as_ref().unwrap().unbounded_send(ni_ty::protocol::GameMessageC2S::SetSettings { settings: new_settings.clone() }.into()).unwrap();

                                                            close = true;
                                                        }

                                                        if ui.button("Cancel").clicked() {
                                                            close = true;
                                                        }
                                                    });
                                                }
                                            );
                                        });
                                    });

                                if close {
                                    self.editing_game_settings = None;
                                }
                            }

                            if self.show_settings {
                                if !render_settings_window(egui_ctx, &ctx) {
                                    self.show_settings = false;
                                }
                            }
                        });
                        let ui_scale = ui_scale.unwrap();

                        egui_macroquad::draw();

                        for (player_id, state) in &shared.menu_mouse_states {
                            let pos = state.get_pos() * ui_scale;
                            ctx.draw_cursor(pos[0], pos[1], *player_id);
                        }

                        self.into()
                    }
                }
                Some(hand) => IngameHandView::new(
                    hand.players()
                        .iter()
                        .position(|player| player.player_id() == shared.my_player_id),
                    self.show_settings,
                )
                .into(),
            }
        } else {
            View::from_connection_state(&lock, ctx)
        }
    }

    fn should_clear_last_menu_mouse_position(&self) -> bool {
        self.show_settings
    }
}

pub struct IngameEndView {
    scores: Vec<(u8, i32)>,
}

impl ViewImpl for IngameEndView {
    fn tick(self, ctx: &mut GameContext) -> View {
        mq::clear_background(BACKGROUND_COLOR);

        let mut lock = ctx.game_info_mutex.lock().unwrap();
        if let Some(shared) = (*lock).as_info_mut() {
            match &shared.game.hand {
                None => {
                    let mut go_next = false;

                    egui_macroquad::ui(|egui_ctx| {
                        egui::CentralPanel::default()
                            .frame(egui::Frame::none())
                            .show(egui_ctx, |ui| {
                                let ui_screen_width = mq::screen_width() / egui_ctx.zoom_factor();
                                let ui_screen_height = mq::screen_height() / egui_ctx.zoom_factor();

                                let row_height = 25.0;

                                let box_width = 250.0;
                                let box_height = (row_height + ui.spacing().item_spacing.y)
                                    * (self.scores.len() as f32)
                                    + row_height;

                                let box_x = ui_screen_width / 2.0 - box_width / 2.0;
                                let box_y = ui_screen_height / 2.0 - box_height / 2.0;

                                ui.allocate_ui_at_rect(
                                    egui::Rect {
                                        min: egui::Pos2::new(box_x, box_y),
                                        max: egui::Pos2::new(box_x + box_width, box_y + box_height),
                                    },
                                    |ui| {
                                        egui::Grid::new("end_scores")
                                            .min_row_height(row_height)
                                            .show(ui, |ui| {
                                                for (player_id, score) in self.scores.iter() {
                                                    ui.label(score.to_string());

                                                    if let Some(player) =
                                                        shared.game.players.get(player_id)
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
                        IngameNeutralView::default().into()
                    } else {
                        self.into()
                    }
                }
                Some(hand) => IngameHandView::new(
                    hand.players()
                        .iter()
                        .position(|player| player.player_id() == shared.my_player_id),
                    false,
                )
                .into(),
            }
        } else {
            View::from_connection_state(&lock, ctx)
        }
    }
}

pub struct LostConnectionView {
    was_connected: bool,
    error: Option<String>,
}

impl ViewImpl for LostConnectionView {
    fn tick(self, ctx: &mut GameContext) -> View {
        mq::clear_background(BACKGROUND_COLOR);

        let screen_center = (mq::screen_width() / 2.0, mq::screen_height() / 2.0);

        ctx.draw_text_centered(
            if self.was_connected {
                "Lost connection to the server."
            } else {
                "Failed to connect to the server."
            },
            screen_center.0,
            screen_center.1,
            60,
            mq::BLACK,
        );

        let mut go_back = false;

        egui_macroquad::ui(|egui_ctx| {
            let screen_margin = get_screen_margin(egui_ctx);

            egui::CentralPanel::default()
                .frame(egui::Frame::none())
                .show(egui_ctx, |ui| {
                    ui.allocate_ui_at_rect(
                        egui::Rect {
                            min: egui::Pos2::new(
                                screen_margin.left,
                                (screen_center.1 + 80.0) / egui_ctx.zoom_factor(),
                            ),
                            max: egui::Pos2::new(
                                mq::screen_width() / egui_ctx.zoom_factor() - screen_margin.right,
                                mq::screen_height() / egui_ctx.zoom_factor(),
                            ),
                        },
                        |ui| {
                            if let Some(error) = &self.error {
                                ui.label(error);
                            }

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
            MainMenuView::init(ctx).into()
        } else {
            self.into()
        }
    }
}

pub struct CreditsView;

impl ViewImpl for CreditsView {
    fn tick(self, ctx: &mut GameContext) -> View {
        let mut go_back = false;

        mq::clear_background(BACKGROUND_COLOR);

        egui_macroquad::ui(|egui_ctx| {
            egui::CentralPanel::default()
                .frame(egui::Frame::none().inner_margin(egui::Margin::same(BASE_SCREEN_MARGIN)))
                .show(egui_ctx, |ui| {
                    if ui.button("Back").clicked() {
                        go_back = true;
                    }

                    egui::ScrollArea::vertical()
                        .auto_shrink(false)
                        .show(ui, |ui| {
                            ui.heading("Credits");

                            ui.label("nertsio by phygs");
                            ui.label("using third-party packages:");

                            for (crate_name, license_text) in crate::licenses::LICENSES {
                                ui.collapsing(*crate_name, |ui| {
                                    ui.label(*license_text);
                                });
                            }
                        });
                });
        });

        egui_macroquad::draw();

        go_back = go_back || mq::is_key_pressed(mq::KeyCode::Escape);

        if go_back {
            MainMenuView::init(ctx).into()
        } else {
            self.into()
        }
    }
}

fn handle_input_response(res: egui::Response) {
    if res.lost_focus() {
        miniquad::window::show_keyboard(false);
    } else if res.gained_focus() {
        miniquad::window::show_keyboard(true);
    }
}

fn get_screen_margin(egui_ctx: &egui::Context) -> egui::Margin {
    let insets = macroquad::miniquad::window::get_safe_insets();

    let scale = egui_ctx.zoom_factor() * 2.0; // no idea why we need this

    egui::Margin {
        left: ((insets.left as f32) / scale) + BASE_SCREEN_MARGIN,
        right: ((insets.right as f32) / scale) + BASE_SCREEN_MARGIN,
        top: ((insets.top as f32) / scale) + BASE_SCREEN_MARGIN,
        bottom: ((insets.bottom as f32) / scale) + BASE_SCREEN_MARGIN,
    }
}
