use crate::settings::DragMode;
use crate::{ConnectionEvent, ConnectionMessage, ConnectionState, Settings};
use futures_util::FutureExt;
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
const SCREEN_MARGIN: f32 = 10.0;
const CARD_SIZE: mq::Vec2 = mq::Vec2 {
    x: metrics::CARD_WIDTH,
    y: metrics::CARD_HEIGHT,
};

const CAN_QUIT: bool = cfg!(not(any(target_family = "wasm", target_os = "android")));

#[allow(clippy::enum_variant_names)]
#[enum_dispatch::enum_dispatch]
pub enum View {
    CreditsView,
    MainMenuView,
    JoinGameFormView,
    PublicGameListLoadingView,
    PublicGameListView,
    ConnectingView,
    IngameNeutralView,
    IngameHandView,
    IngameEndView,
    LostConnectionView,
}

#[enum_dispatch::enum_dispatch(View)]
pub trait ViewImpl {
    fn tick(self, ctx: &mut GameContext) -> View;
}

impl View {
    pub fn from_connection_state(src: &ConnectionState) -> Self {
        match src {
            ConnectionState::NotConnected {
                expected: true,
                code: _,
            } => MainMenuView::default().into(),
            ConnectionState::NotConnected {
                expected: false,
                code,
            } => LostConnectionView {
                was_connected: true,
                code: *code,
            }
            .into(),
            _ => LostConnectionView {
                was_connected: true,
                code: None,
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

    pub cards_texture: &'a mq::Texture2D,
    pub backs_texture: mq::Texture2D,
    pub cursors_texture: mq::Texture2D,
    pub placeholder_texture: mq::Texture2D,
    pub font: mq::Font,
    pub nerts_callout: &'a macroquad::audio::Sound,
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
    }

    fn start_loading_public_games(
        &self,
    ) -> futures_channel::oneshot::Receiver<Vec<ni_ty::protocol::PublicGameInfoExpanded<'static>>>
    {
        let (send, recv) = futures_channel::oneshot::channel();

        let req_fut = self
            .http_client
            .get(format!("{}public_games", self.coordinator_url))
            .send();
        self.async_rt.spawn(
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
            None,
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
}

pub fn render_settings_window(egui_ctx: &egui::Context, settings_mutex: &Mutex<Settings>) -> bool {
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

                        ui.checkbox(&mut settings.round_start_music, "Round Start Music");
                        ui.checkbox(&mut settings.suit_callouts, "Suit Callouts");
                        ui.checkbox(&mut settings.nerts_callout, "Nerts Callout");
                    },
                );
            });
        });

    open
}

#[derive(Default)]
pub struct MainMenuView {
    show_settings: bool,
}

impl ViewImpl for MainMenuView {
    fn tick(mut self, ctx: &mut GameContext) -> View {
        mq::clear_background(BACKGROUND_COLOR);

        let button_height = 20.0;

        let button_count = 7;

        let menu_width = 150.0;

        let mut new_state: Option<View> = None;

        egui_macroquad::ui(|egui_ctx| {
            egui::CentralPanel::default()
                .frame(egui::Frame::none())
                .show(egui_ctx, |ui| {
                    if self.show_settings {
                        ui.disable();
                    }

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
                            ui.with_layout(egui::Layout::top_down(egui::Align::Center), |ui| {
                                ui.heading("nertsio");
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

                            if menu_button(ui, "Create Public Game") {
                                ctx.do_connection(crate::connection::ConnectionType::CreateGame {
                                    public: true,
                                });
                                new_state = Some(ConnectingView.into());
                            } else if menu_button(ui, "Create Private Game") {
                                ctx.do_connection(crate::connection::ConnectionType::CreateGame {
                                    public: false,
                                });
                                new_state = Some(ConnectingView.into());
                            } else if menu_button(ui, "Join Public Game") {
                                let channel = ctx.start_loading_public_games();
                                new_state = Some(PublicGameListLoadingView { channel }.into());
                            } else if menu_button(ui, "Join Private Game") {
                                new_state = Some(JoinGameFormView::default().into());
                            } else if menu_button(ui, "Settings") {
                                self.show_settings = true;
                            } else if CAN_QUIT && menu_button(ui, "Quit") {
                                ctx.quit();
                            }
                        },
                    );
                });

            egui::TopBottomPanel::bottom("main_menu_bottom")
                .show_separator_line(false)
                .frame(egui::Frame::none().inner_margin(egui::Margin::same(SCREEN_MARGIN)))
                .show(egui_ctx, |ui| {
                    if ui.button("Credits").clicked() {
                        new_state = Some(CreditsView.into());
                    }
                });

            if self.show_settings {
                if !render_settings_window(egui_ctx, &ctx.settings_mutex) {
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
            MainMenuView::default().into()
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
            ConnectionState::NotConnected { expected, code } => {
                if expected {
                    MainMenuView::default().into()
                } else {
                    LostConnectionView {
                        was_connected: false,
                        code,
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
        mq::clear_background(BACKGROUND_COLOR);

        let mut lock = ctx.game_info_mutex.lock().unwrap();
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

                        egui_macroquad::ui(|egui_ctx| {
                            egui::SidePanel::right("game_settings_panel")
                                .frame(
                                    egui::Frame::none()
                                        .inner_margin(egui::Margin::same(SCREEN_MARGIN)),
                                )
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
                                                ui.label(format!("Region: {}", region.id));
                                            }

                                            if let Some(ping) = &shared.ping {
                                                ui.label(format!("Ping: {}ms", ping.as_millis()));
                                            }
                                        },
                                    );
                                });

                            egui::CentralPanel::default().frame(egui::Frame::none().inner_margin(egui::Margin::same(SCREEN_MARGIN))).show(egui_ctx, |ui| {
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
                                if !render_settings_window(egui_ctx, &ctx.settings_mutex) {
                                    self.show_settings = false;
                                }
                            }
                        });

                        egui_macroquad::draw();

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
            View::from_connection_state(&lock)
        }
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
            View::from_connection_state(&lock)
        }
    }
}

pub struct LostConnectionView {
    was_connected: bool,
    code: Option<u8>,
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

        if let Some(details) = match self.code {
            Some(ni_ty::protocol::CLOSE_KICK) => Some("Kicked by master"),
            Some(ni_ty::protocol::CLOSE_TOO_NEW) => Some("Server is too old"),
            Some(ni_ty::protocol::CLOSE_TOO_OLD) => {
                Some("Your client is too old to connect to this server")
            }
            _ => None,
        } {
            ctx.draw_text_centered(
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
            MainMenuView::default().into()
        } else {
            self.into()
        }
    }
}

pub struct PublicGameListLoadingView {
    channel:
        futures_channel::oneshot::Receiver<Vec<ni_ty::protocol::PublicGameInfoExpanded<'static>>>,
}

impl ViewImpl for PublicGameListLoadingView {
    fn tick(mut self, ctx: &mut GameContext) -> View {
        mq::clear_background(BACKGROUND_COLOR);

        let screen_center = (mq::screen_width() / 2.0, mq::screen_height() / 2.0);

        ctx.draw_text_centered(
            "Loading...",
            screen_center.0,
            screen_center.1,
            60,
            mq::BLACK,
        );

        if mq::is_key_pressed(mq::KeyCode::Escape) {
            MainMenuView::default().into()
        } else {
            match self.channel.try_recv() {
                Ok(Some(list)) => PublicGameListView { list }.into(),
                Ok(None) => self.into(),
                Err(futures_channel::oneshot::Canceled) => MainMenuView::default().into(),
            }
        }
    }
}

pub struct PublicGameListView {
    list: Vec<ni_ty::protocol::PublicGameInfoExpanded<'static>>,
}

impl ViewImpl for PublicGameListView {
    fn tick(self, ctx: &mut GameContext) -> View {
        mq::clear_background(BACKGROUND_COLOR);

        let screen_center = (mq::screen_width() / 2.0, mq::screen_height() / 2.0);

        let menu_width = 250.0;

        let mut go_back = false;

        let mut joining = None;

        egui_macroquad::ui(|egui_ctx| {
            egui::CentralPanel::default()
                .frame(egui::Frame::none().inner_margin(egui::Margin::same(SCREEN_MARGIN)))
                .show(egui_ctx, |ui| {
                    if ui.button("Back").clicked() {
                        go_back = true;
                    }

                    if !self.list.is_empty() {
                        ui.vertical_centered(|ui| {
                            ui.heading("Public Games");
                            ui.allocate_ui_with_layout(
                                egui::Vec2::new(menu_width, 0.0),
                                egui::Layout::top_down(egui::Align::Min),
                                |ui| {
                                    egui::Grid::new("public_game_list").show(ui, |ui| {
                                        for game in &self.list {
                                            ui.label(crate::util::to_full_game_id_str(
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

        if self.list.is_empty() {
            ctx.draw_text_centered(
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
                    MainMenuView::default().into()
                } else {
                    self.into()
                }
            }
            Some(game) => {
                ctx.do_connection(crate::connection::ConnectionType::JoinPublicGame {
                    server: game.server.clone(),
                    game_id: game.game_id,
                });
                ConnectingView.into()
            }
        }
    }
}

pub struct CreditsView;

impl ViewImpl for CreditsView {
    fn tick(self, _ctx: &mut GameContext) -> View {
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
            MainMenuView::default().into()
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
