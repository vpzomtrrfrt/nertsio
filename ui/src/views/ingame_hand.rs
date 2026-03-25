use super::ingame_hand_common;
use crate::ConnectionMessage;
use macroquad::prelude as mq;
use nertsio_types as ni_ty;
use nertsio_ui_metrics as metrics;
use std::borrow::Cow;

const START_ANIMATION_SPEED: f32 = 1500.0;

pub struct IngameHandView {
    my_player_idx: Option<usize>,
    show_settings: bool,
    start_animation_progress: f32,
}

impl IngameHandView {
    pub fn new(my_player_idx: Option<usize>, show_settings: bool) -> Self {
        Self {
            my_player_idx,
            show_settings,
            start_animation_progress: 0.0,
        }
    }
}

impl super::ViewImpl for IngameHandView {
    fn tick(mut self, ctx: &mut super::GameContext) -> super::View {
        let interaction_enabled = !self.show_settings;

        let mut lock = ctx.game_info_mutex.lock().unwrap();
        if let Some(shared) = (*lock).as_info_mut() {
            if let Some(real_hand_state) = shared.game.hand.as_mut() {
                let started = real_hand_state.started;
                let hand_extra = shared.hand_extra.as_mut().unwrap();

                let metrics = ingame_hand_common::hand_metrics(real_hand_state, 13);

                let real_screen_size = (mq::screen_width(), mq::screen_height());
                let screen_size =
                    ingame_hand_common::screen_size_for_hand(real_screen_size, &metrics);
                let scale = real_screen_size.0 / screen_size.0;

                let camera_rect = mq::Rect::new(0.0, screen_size.1, screen_size.0, -screen_size.1);

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

                let (pred_hand_state, self_inverted) =
                    if let Some(my_player_idx) = self.my_player_idx {
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
                            let mut settings_lock = ctx.settings_mutex.lock().unwrap();
                            let settings = &mut *settings_lock;

                            if let Some(action) = ingame_hand_common::handle_input(
                                ctx,
                                settings,
                                &metrics,
                                screen_center.into(),
                                my_player_idx,
                                &pred_hand_state,
                                &mut hand_extra.my_held_state,
                                mouse_pos,
                            ) {
                                match pred_hand_state.apply(Some(my_player_idx_u8), action) {
                                    Ok(_) => {
                                        hand_extra.pending_actions.push_back(action);
                                        ctx.game_msg_send
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
                                    }
                                    Err(_) => {
                                        eprintln!("Failed to apply player action");
                                    }
                                }

                                if settings.sounds {
                                    ctx.play_sound_for_action(action);
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

                mq::clear_background(super::BACKGROUND_COLOR);

                let hand_scores = pred_hand_state.calculate_hand_scores(&shared.game.settings);

                for (idx, player_state) in pred_hand_state.players().iter().enumerate() {
                    let hand_score = hand_scores[idx];

                    let player = match shared.game.players.get(&player_state.player_id()) {
                        Some(player) => player,
                        None => continue,
                    };

                    let location = metrics.player_loc(idx);
                    let position = mq::Vec2::from(location.pos()) + mq::Vec2::from(screen_center);

                    mq::set_camera(&normal_camera);

                    let name_pos = if location.inverted == self_inverted {
                        (
                            position[0] + metrics.player_hand_width() / 2.0,
                            position[1] - 50.0,
                        )
                    } else {
                        (
                            screen_center.0 - location.x - metrics.player_hand_width() / 2.0,
                            screen_center.1 - metrics::PLAYER_Y + 50.0,
                        )
                    };

                    let score_pos = if location.inverted == self_inverted {
                        (name_pos.0, name_pos.1 + 35.0)
                    } else {
                        (name_pos.0, name_pos.1 - 35.0)
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

                    ctx.draw_text_centered(&player.name, name_pos.0, name_pos.1, 40, mq::BLACK);

                    ctx.draw_text_centered(
                        &if hand_score < 0 {
                            format!("{} - {}", player.score, -hand_score)
                        } else {
                            format!("{} + {}", player.score, hand_score)
                        },
                        score_pos.0,
                        score_pos.1,
                        30,
                        mq::BLACK,
                    );

                    if location.inverted != self_inverted {
                        mq::set_camera(&inverted_camera);
                    }

                    let held_info = if Some(idx) == self.my_player_idx {
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
                                        let cards = &cards[(cards.len() - held.count as usize)..];

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

                    ingame_hand_common::draw_player_stacks(
                        ctx,
                        player_state,
                        &held_info,
                        &metrics,
                        location,
                        screen_center.into(),
                        started,
                        self.start_animation_progress,
                    );

                    if hand_extra.stalled {
                        let stock_pos = mq::Vec2::from(
                            metrics.player_stack_pos(ni_ty::PlayerStackLocation::Stock, location),
                        ) + mq::Vec2::from(screen_center);

                        ctx.draw_text(
                            "Shuffling soon if game remains stalled...",
                            stock_pos[0],
                            stock_pos[1]
                                + metrics::CARD_HEIGHT
                                + 15.0
                                + metrics::NOTICE_HEIGHT / 2.0,
                            metrics::NOTICE_FONT_SIZE,
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
                    let pos =
                        mq::Vec2::from(metrics.stack_pos(loc)) + mq::Vec2::from(screen_center);

                    match stack.cards().last() {
                        None => {
                            ctx.draw_placeholder(pos[0], pos[1]);
                        }
                        Some(card) => {
                            ctx.draw_card(card.card, pos[0], pos[1]);
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
                            ingame_hand_common::draw_held_state(
                                ctx,
                                &pred_hand_state,
                                idx,
                                held,
                                mq::Vec2::from(screen_center) + mouse_pos,
                            );
                        }

                        ctx.draw_cursor(
                            screen_center.0 + mouse_pos[0] - 1.0,
                            screen_center.1 + mouse_pos[1] - 1.0,
                            pred_hand_state.players()[idx].player_id(),
                        );
                    }
                }

                mq::set_camera(&normal_camera);

                {
                    let mut pending_players_iter = shared.game.players.iter().filter(|(id, _)| {
                        !pred_hand_state
                            .players()
                            .iter()
                            .any(|x| x.player_id() == **id)
                    });
                    if let Some(first_pending_player) = pending_players_iter.next() {
                        let count = pending_players_iter.count() + 1;
                        let msg = if count == 1 {
                            format!("1 spectator: {}", first_pending_player.1.name)
                        } else {
                            format!("{} spectators", count)
                        };

                        ctx.draw_text_centered(
                            &msg,
                            screen_center.0,
                            metrics::NOTICE_HEIGHT / 2.0,
                            metrics::NOTICE_FONT_SIZE,
                            mq::BLACK,
                        );
                    }
                }

                if let Some(my_player_idx) = self.my_player_idx {
                    let my_player_state = &pred_hand_state.players()[my_player_idx];
                    if let Some(ref held) = hand_extra.my_held_state {
                        ingame_hand_common::draw_held_state(
                            ctx,
                            &pred_hand_state,
                            my_player_idx,
                            held.info,
                            mouse_pos,
                        );

                        let stack = my_player_state.stack_at(held.info.src);
                        if let Some(stack) = stack {
                            let stack_cards = stack.cards();
                            if stack_cards.len() < held.info.count as usize {
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
                        ingame_hand_common::NERTS_OVERLAY_COLOR,
                    );

                    ctx.draw_text_centered(
                        "Nerts!",
                        screen_center.0,
                        screen_center.1,
                        100,
                        ingame_hand_common::NERTS_TEXT_COLOR,
                    );
                }

                if !started {
                    mq::draw_rectangle(
                        0.0,
                        screen_center.1 - 70.0,
                        screen_size.0,
                        140.0,
                        ingame_hand_common::NERTS_OVERLAY_COLOR,
                    );

                    if let Some(expected_start_time) = hand_extra.expected_start_time {
                        if let Some(time_until) =
                            expected_start_time.checked_duration_since(web_time::Instant::now())
                        {
                            ctx.draw_text_centered(
                                &(time_until.as_secs() + 1).to_string(),
                                screen_center.0,
                                screen_center.1,
                                100,
                                ingame_hand_common::NERTS_TEXT_COLOR,
                            );
                        }
                    }
                }

                egui_macroquad::ui(|egui_ctx| {
                    let ui_scale = scale / egui_ctx.zoom_factor();

                    egui::CentralPanel::default()
                        .frame(
                            egui::Frame::none()
                                .inner_margin(egui::Margin::same(super::SCREEN_MARGIN)),
                        )
                        .show(egui_ctx, |ui| {
                            if !interaction_enabled {
                                ui.disable();
                            }

                            ui.with_layout(egui::Layout::left_to_right(egui::Align::Min), |ui| {
                                if ui.button("Leave").clicked() {
                                    ctx.game_msg_send
                                        .borrow()
                                        .as_ref()
                                        .unwrap()
                                        .unbounded_send(ConnectionMessage::Leave)
                                        .unwrap();
                                }

                                if ui.button("Settings").clicked() {
                                    self.show_settings = true;
                                }

                                if let Some(ping) = shared.ping {
                                    ui.label(format!("Ping: {}ms", ping.as_millis()));
                                }
                            });

                            ui.label(format!(
                                "Room Code: {}",
                                crate::util::to_full_game_id_str(shared.server_id, shared.game.id)
                            ));

                            if let Some(my_player_idx) = self.my_player_idx {
                                let my_player_state = &pred_hand_state.players()[my_player_idx];

                                if my_player_state.nerts_stack().is_empty() {
                                    let location = metrics.player_loc(my_player_idx);
                                    let position = mq::Vec2::from(location.pos()) + mq::Vec2::from(screen_center);

                                    ui.allocate_ui_at_rect(
                                        egui::Rect {
                                            min: egui::Pos2::new(position[0], position[1]),
                                            max: egui::Pos2::new(
                                                position[0] + 12.0 * metrics::NERTS_STACK_SPACING + metrics::CARD_WIDTH,
                                                position[1] + metrics::CARD_HEIGHT,
                                            )
                                        } * ui_scale,
                                        |ui| {
                                            ui.centered_and_justified(|ui| {
                                                if ui.button("Nerts!").clicked() {
                                                    hand_extra.self_called_nerts = true;
                                                    ctx.game_msg_send
                                                        .borrow()
                                                        .as_ref()
                                                        .unwrap()
                                                        .unbounded_send(
                                                            ni_ty::protocol::GameMessageC2S::CallNerts.into(),
                                                        )
                                                        .unwrap();

                                                    let mut settings_lock = ctx.settings_mutex.lock().unwrap();
                                                    let settings = &mut *settings_lock;

                                                    if settings.sounds {
                                                        macroquad::audio::play_sound_once(ctx.nerts_callout);
                                                    }
                                                }
                                            });
                                        },
                                    );
                                }
                            }
                        });

                    if self.show_settings {
                        if !super::render_settings_window(egui_ctx, ctx) {
                            self.show_settings = false;
                        }
                    }
                });

                egui_macroquad::draw();

                if !started {
                    self.start_animation_progress += mq::get_frame_time() * START_ANIMATION_SPEED;
                }

                self.into()
            } else {
                super::IngameNeutralView {
                    show_settings: self.show_settings,
                    editing_game_settings: None,
                }
                .into()
            }
        } else {
            super::View::from_connection_state(&lock, ctx)
        }
    }
}
