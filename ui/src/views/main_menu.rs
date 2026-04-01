use crate::views;
use macroquad::prelude as mq;
use nertsio_common as common;
use nertsio_types as ni_ty;

const PUBLIC_GAMES_REFRESH_DELAY: std::time::Duration = std::time::Duration::from_secs(5);

const CAN_QUIT: bool = cfg!(not(any(
    target_family = "wasm",
    target_os = "android",
    target_os = "ios"
)));

pub struct MainMenuView {
    show_settings: bool,

    public_games_state: crate::LoadState<Vec<ni_ty::protocol::PublicGameInfoExpanded<'static>>>,
    public_games_done_at: Option<web_time::Instant>,
    public_games_reload_channel:
        Option<crate::LoadChannel<Vec<ni_ty::protocol::PublicGameInfoExpanded<'static>>>>,
}

impl MainMenuView {
    pub fn init(ctx: &views::GameContext) -> MainMenuView {
        Self {
            show_settings: false,
            public_games_state: ctx.start_loading_public_games().into(),
            public_games_done_at: None,
            public_games_reload_channel: None,
        }
    }
}

impl views::ViewImpl for MainMenuView {
    fn tick(mut self, ctx: &mut views::GameContext) -> views::View {
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

        mq::clear_background(views::BACKGROUND_COLOR);

        let button_height = 20.0;

        let button_count = 7;

        let menu_width = 150.0;

        let mut new_state: Option<views::View> = None;

        egui_macroquad::ui(|egui_ctx| {
            egui::SidePanel::left("main_menu")
                .frame(egui::Frame::none().inner_margin(egui::Margin::same(views::SCREEN_MARGIN)))
                .show(egui_ctx, |ui| {
                    if self.show_settings {
                        ui.disable();
                    }

                    let menu_height = button_height * (button_count as f32)
                        + ((button_count - 1) as f32) * ui.spacing().item_spacing.y
                        + 60.0
                        + 5.0;

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
                                    views::handle_input_response(
                                        ui.add(
                                            egui::widgets::TextEdit::singleline(&mut settings.name)
                                                .char_limit(common::MAX_NAME_LENGTH),
                                        ),
                                    );
                                });
                            }

                            if menu_button(ui, "Create New Game") {
                                ctx.do_connection(crate::connection::ConnectionType::CreateGame {});
                                new_state = Some(views::ConnectingView.into());
                            } else if menu_button(ui, "Join Private Game") {
                                new_state = Some(views::JoinGameFormView::default().into());
                            } else if menu_button(ui, "Practice") {
                                new_state = Some(views::PracticeSetupView::default().into());
                            } else {
                                ui.add_space(5.0);

                                if menu_button(ui, "Settings") {
                                    self.show_settings = true;
                                } else if menu_button(ui, "Credits") {
                                    new_state = Some(views::CreditsView.into());
                                } else if CAN_QUIT && menu_button(ui, "Quit") {
                                    ctx.quit();
                                }
                            }
                        },
                    );
                });

            egui::CentralPanel::default()
                .frame(egui::Frame::none().inner_margin(egui::Margin::same(views::SCREEN_MARGIN)))
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
                                            new_state = Some(views::ConnectingView.into());
                                        }

                                        ui.end_row();
                                    }
                                }
                            });
                        }
                    }
                });

            if self.show_settings {
                if !views::render_settings_window(egui_ctx, ctx) {
                    self.show_settings = false;
                }
            }
        });

        egui_macroquad::draw();

        new_state.unwrap_or(self.into())
    }
}
