use crate::views;
use macroquad::hash;
use macroquad::prelude as mq;
use nertsio_types as ni_ty;

mod hand;

pub use hand::IngameHandView;

#[derive(Default)]
pub struct IngameNeutralView {
    show_settings: bool,
    editing_game_settings: Option<ni_ty::GameSettings>,
}

impl views::ViewImpl for IngameNeutralView {
    fn tick(mut self, ctx: &mut views::GameContext) -> views::View {
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

        mq::clear_background(views::BACKGROUND_COLOR);

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
                            ui_scale = Some(egui_ctx.zoom_factor());

                            egui::SidePanel::right("game_settings_panel")
                                .frame(
                                    egui::Frame::none()
                                        .inner_margin(egui::Margin::same(views::SCREEN_MARGIN)),
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

                                            ui.label(format!(
                                                "Nerts Stack Size: {}",
                                                shared.game.settings.nerts_stack_size,
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

                            egui::CentralPanel::default().frame(egui::Frame::none().inner_margin(egui::Margin::same(views::SCREEN_MARGIN))).show(egui_ctx, |ui| {
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

                                                    ui.label("Nerts Stack Size");
                                                    ui.indent(hash!(), |ui| {
                                                        ui.add(egui::widgets::Slider::new(
                                                            &mut new_settings.nerts_stack_size,
                                                            0..=46,
                                                        ));
                                                    });

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
                                if !views::render_settings_window(egui_ctx, ctx) {
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
            views::View::from_connection_state(&lock, ctx)
        }
    }

    fn should_clear_last_menu_mouse_position(&self) -> bool {
        self.show_settings
    }
}

pub struct IngameEndView {
    scores: Vec<(u8, i32)>,
}

impl views::ViewImpl for IngameEndView {
    fn tick(self, ctx: &mut views::GameContext) -> views::View {
        mq::clear_background(views::BACKGROUND_COLOR);

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
            views::View::from_connection_state(&lock, ctx)
        }
    }
}
