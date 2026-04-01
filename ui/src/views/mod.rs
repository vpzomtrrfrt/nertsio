use crate::settings::DragMode;
use crate::{ConnectionEvent, ConnectionMessage, ConnectionState, Settings};
use macroquad::hash;
use macroquad::logging as log;
use macroquad::miniquad;
use macroquad::prelude as mq;
use nertsio_types as ni_ty;
use nertsio_ui_metrics as metrics;
use std::cell::RefCell;
use std::sync::{Arc, Mutex};

mod ingame;
mod ingame_hand_common;
mod main_menu;
mod practice;

pub use ingame::{IngameEndView, IngameHandView, IngameNeutralView};
pub use main_menu::MainMenuView;
pub use practice::{PracticeEndView, PracticeHandView, PracticeSetupView};

const BACKGROUND_COLOR: mq::Color = mq::Color::new(0.1, 0.6, 0.1, 1.0);
const SCREEN_MARGIN: f32 = 5.0;
const CARD_SIZE: mq::Vec2 = mq::Vec2 {
    x: metrics::CARD_WIDTH,
    y: metrics::CARD_HEIGHT,
};

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
    PracticeEndView,
    PracticeHandView,
    PracticeSetupView,
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
    pub storage: Option<Arc<crate::storage::DefaultStorage>>,
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

    pub suit_callout_spades: &'a macroquad::audio::Sound,
    pub suit_callout_diamonds: &'a macroquad::audio::Sound,
    pub suit_callout_clubs: &'a macroquad::audio::Sound,
    pub suit_callout_hearts: &'a macroquad::audio::Sound,
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

    pub fn play_sound_for_action(&self, action: ni_ty::HandAction, is_me: bool) {
        #[allow(unreachable_patterns)]
        let sound = match action {
            ni_ty::HandAction::ReturnStock => Some(&self.gather_sound),
            ni_ty::HandAction::FlipStock => Some(&self.flip_sound),
            ni_ty::HandAction::Move { .. } => Some(&self.place_sound),
            ni_ty::HandAction::ShuffleStock { .. } => Some(&self.shuffle_sound),
            _ => None,
        };

        if let Some(sound) = sound {
            let volume = match action {
                ni_ty::HandAction::FlipStock | ni_ty::HandAction::ReturnStock => {
                    if is_me {
                        0.5
                    } else {
                        0.1
                    }
                }

                ni_ty::HandAction::Move {
                    to: ni_ty::StackLocation::Lake(_),
                    ..
                } => 1.0,
                ni_ty::HandAction::Move { .. } => {
                    if is_me {
                        1.0
                    } else {
                        0.5
                    }
                }

                _ => 1.0,
            };

            macroquad::audio::play_sound(
                sound,
                macroquad::audio::PlaySoundParams {
                    looped: false,
                    volume,
                },
            );
        }
    }

    pub fn play_sound_for_new_lake_stack(&self, card: ni_ty::Card) {
        macroquad::audio::play_sound_once(match card.suit {
            ni_ty::Suit::Spades => &self.suit_callout_spades,
            ni_ty::Suit::Diamonds => &self.suit_callout_diamonds,
            ni_ty::Suit::Clubs => &self.suit_callout_clubs,
            ni_ty::Suit::Hearts => &self.suit_callout_hearts,
        });
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
            egui::CentralPanel::default()
                .frame(egui::Frame::none().inner_margin(egui::Margin::same(SCREEN_MARGIN)))
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
            egui::CentralPanel::default()
                .frame(egui::Frame::none())
                .show(egui_ctx, |ui| {
                    ui.allocate_ui_at_rect(
                        egui::Rect {
                            min: egui::Pos2::new(
                                SCREEN_MARGIN,
                                (screen_center.1 + 80.0) / egui_ctx.zoom_factor(),
                            ),
                            max: egui::Pos2::new(
                                (mq::screen_width() - SCREEN_MARGIN) / egui_ctx.zoom_factor(),
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
                .frame(egui::Frame::none().inner_margin(egui::Margin::same(SCREEN_MARGIN)))
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
