use super::ingame_hand_common;
use crate::storage::Storage;
use macroquad::logging as log;
use macroquad::prelude as mq;
use nertsio_types as ni_ty;
use std::collections::HashMap;
use strum::IntoEnumIterator;

const START_ANIMATION_SPEED: f32 = 5000.0;
const START_TIME: std::time::Duration = std::time::Duration::from_secs(3);

#[derive(Clone, Copy, PartialEq, Debug, strum_macros::EnumIter, strum_macros::Display)]
pub enum PracticeSpec {
    Invert,
    Distribute,
}

impl PracticeSpec {
    pub fn gen_hand(&self) -> ni_ty::HandState {
        match self {
            PracticeSpec::Invert => ni_ty::HandState::raw(
                vec![ni_ty::HandPlayerState::raw(
                    0,
                    ni_ty::Stack::from_list_unordered({
                        let mut list: Vec<_> = ni_ty::Rank::iter()
                            .map(|rank| {
                                ni_ty::CardInstance::new(
                                    ni_ty::Card::new(ni_ty::Suit::Hearts, rank),
                                    0,
                                )
                            })
                            .collect();
                        list.reverse();
                        list
                    }),
                    ni_ty::Stack::new(ni_ty::Ordering::Any, false),
                    ni_ty::Stack::new(ni_ty::Ordering::Any, false),
                    vec![],
                )],
                vec![lake_stack()],
            ),
            PracticeSpec::Distribute => ni_ty::HandState::raw(
                vec![ni_ty::HandPlayerState::raw(
                    0,
                    ni_ty::Stack::from_list_unordered({
                        let mut next_rank_map: HashMap<_, _> = ni_ty::Suit::iter()
                            .map(|suit| (suit, ni_ty::Rank::KING))
                            .collect();

                        let mut list_src: Vec<_> = ni_ty::Suit::iter()
                            .flat_map(|suit| std::iter::repeat_n(suit, ni_ty::Rank::COUNT.into()))
                            .collect();
                        rand::seq::SliceRandom::shuffle(&mut list_src[..], &mut rand::thread_rng());

                        let mut list = Vec::with_capacity(list_src.len());
                        for suit in list_src {
                            let rank = next_rank_map.get(&suit).unwrap();
                            list.push(ni_ty::CardInstance::new(ni_ty::Card::new(suit, *rank), 0));

                            if let Some(next_rank) = rank.decrement() {
                                next_rank_map.insert(suit, next_rank);
                            } else {
                                next_rank_map.remove(&suit);
                            }
                        }

                        list
                    }),
                    ni_ty::Stack::new(ni_ty::Ordering::Any, false),
                    ni_ty::Stack::new(ni_ty::Ordering::Any, false),
                    vec![],
                )],
                ni_ty::Suit::iter().map(|_| lake_stack()).collect(),
            ),
        }
    }

    pub fn is_done(&self, hand: &ni_ty::HandState) -> bool {
        match self {
            PracticeSpec::Invert | PracticeSpec::Distribute => {
                hand.players()[0].nerts_stack().is_empty()
            }
        }
    }
}

pub struct PracticeHandView {
    spec: PracticeSpec,
    hand: ni_ty::HandState,
    my_held_state: Option<crate::HeldState>,
    start_animation_progress: f32,
    started_at: web_time::Instant,
    time: f32,
}

impl PracticeHandView {
    pub fn new(spec: PracticeSpec) -> Self {
        Self {
            hand: spec.gen_hand(),
            spec,
            my_held_state: None,
            start_animation_progress: 0.0,
            started_at: web_time::Instant::now(),
            time: 0.0,
        }
    }
}

impl super::ViewImpl for PracticeHandView {
    fn tick(mut self, ctx: &mut super::GameContext) -> super::View {
        let hand = &mut self.hand;
        let my_held_state = &mut self.my_held_state;

        let metrics = ingame_hand_common::hand_metrics(
            hand,
            match self.spec {
                PracticeSpec::Invert => ni_ty::Rank::COUNT.into(),
                PracticeSpec::Distribute => usize::from(ni_ty::Rank::COUNT) * 4,
            },
        );

        let real_screen_size = (mq::screen_width(), mq::screen_height());
        let screen_size = ingame_hand_common::screen_size_for_hand(real_screen_size, &metrics);

        let camera_rect = mq::Rect::new(0.0, screen_size.1, screen_size.0, -screen_size.1);

        let normal_camera = mq::Camera2D {
            ..mq::Camera2D::from_display_rect(camera_rect)
        };

        let inverted_camera = mq::Camera2D {
            rotation: 180.0,
            ..mq::Camera2D::from_display_rect(camera_rect)
        };

        let screen_center = (screen_size.0 / 2.0, screen_size.1 / 2.0);

        let started = hand.started;

        mq::clear_background(super::BACKGROUND_COLOR);

        let location = metrics.player_loc(0);

        let mouse_pos = mq::mouse_position();
        let mouse_pos = mq::Vec2::new(
            mouse_pos.0 * screen_size.0 / real_screen_size.0,
            mouse_pos.1 * screen_size.1 / real_screen_size.1,
        );

        if started {
            let mut settings_lock = ctx.settings_mutex.lock().unwrap();
            let settings = &mut *settings_lock;

            if let Some(action) = ingame_hand_common::handle_input(
                ctx,
                settings,
                &metrics,
                screen_center.into(),
                0,
                hand,
                my_held_state,
                mouse_pos,
            ) {
                hand.apply(Some(0), action)
                    .expect("Failed to apply player action");

                if settings.sounds {
                    ctx.play_sound_for_action(action);

                    if let ni_ty::HandAction::Move { to, .. } = action {
                        if matches!(to, ni_ty::StackLocation::Lake(_)) {
                            if let Some(stack) = hand.stack_at(to) {
                                if let Some(top) = stack.last() {
                                    if top.card.rank == ni_ty::Rank::ACE {
                                        ctx.play_sound_for_new_lake_stack(top.card);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        let held_info = my_held_state.as_ref().map(|x| x.info);

        ingame_hand_common::draw_player_stacks(
            ctx,
            &hand.players()[0],
            &held_info,
            &metrics,
            location,
            screen_center.into(),
            started,
            self.start_animation_progress,
        );

        let my_location = metrics.player_loc(0);

        if my_location.inverted {
            mq::set_camera(&inverted_camera);
        } else {
            mq::set_camera(&normal_camera);
        }

        for (i, stack) in hand.lake_stacks().iter().enumerate() {
            let loc = ni_ty::StackLocation::Lake(i as u16);
            let pos = mq::Vec2::from(metrics.stack_pos(loc)) + mq::Vec2::from(screen_center);

            match stack.cards().last() {
                None => {
                    ctx.draw_placeholder(pos[0], pos[1]);
                }
                Some(card) => {
                    ctx.draw_card(card.card, pos[0], pos[1]);
                }
            }
        }

        mq::set_camera(&normal_camera);

        if let Some(held_info) = held_info {
            ingame_hand_common::draw_held_state(ctx, hand, 0, held_info, mouse_pos);
        }

        if !started {
            mq::draw_rectangle(
                0.0,
                screen_center.1 - 70.0,
                screen_size.0,
                140.0,
                ingame_hand_common::NERTS_OVERLAY_COLOR,
            );

            if let Some(time) = web_time::Instant::now().checked_duration_since(self.started_at) {
                if let Some(time_until) = START_TIME.checked_sub(time) {
                    ctx.draw_text_centered(
                        &(time_until.as_secs() + 1).to_string(),
                        screen_center.0,
                        screen_center.1,
                        100,
                        ingame_hand_common::NERTS_TEXT_COLOR,
                    );
                } else {
                    hand.started = true;
                }
            }
        }

        let mut do_leave = false;

        egui_macroquad::ui(|egui_ctx| {
            egui::CentralPanel::default()
                .frame(egui::Frame::none().inner_margin(egui::Margin::same(super::SCREEN_MARGIN)))
                .show(egui_ctx, |ui| {
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Min), |ui| {
                        if ui.button("Leave").clicked() {
                            do_leave = true;
                        }
                    });

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                        if started {
                            ui.label(format!("{:.2}", self.time));
                        }
                    });
                });
        });

        egui_macroquad::draw();

        if started {
            self.time += mq::get_frame_time();
        } else {
            self.start_animation_progress += mq::get_frame_time() * START_ANIMATION_SPEED;
        }

        if do_leave {
            super::MainMenuView::init(ctx).into()
        } else if self.spec.is_done(hand) {
            PracticeEndView::init(ctx, self.spec, self.time).into()
        } else {
            self.into()
        }
    }
}

pub struct PracticeEndView {
    spec: PracticeSpec,
    time: f32,
    previous_best_time: Option<Option<f32>>,
}

impl PracticeEndView {
    fn init(ctx: &super::GameContext, spec: PracticeSpec, time: f32) -> Self {
        let key = format!("practiceBestTime/{:?}", spec);

        let previous_best_time = ctx
            .storage
            .as_ref()
            .and_then(|storage| match storage.get(&key) {
                Err(err) => {
                    eprintln!("Failed to fetch score: {:?}", err);
                    None
                }
                Ok(None) => Some(None),
                Ok(Some(value)) => match value.parse() {
                    Ok(value) => Some(Some(value)),
                    Err(err) => {
                        eprintln!("Failed to fetch score: {:?}", err);
                        Some(None)
                    }
                },
            });

        log::debug!("Previous best score: {:?}", previous_best_time);

        let should_save_time = match previous_best_time {
            Some(None) => true,
            Some(Some(previous_best_time)) => time < previous_best_time,
            _ => false,
        };
        if should_save_time {
            if let Err(err) = ctx.storage.as_ref().unwrap().set(&key, time.to_string()) {
                eprintln!("Failed to save new score: {:?}", err);
            }
        }

        Self {
            spec,
            time,
            previous_best_time,
        }
    }
}

impl super::ViewImpl for PracticeEndView {
    fn tick(self, ctx: &mut super::GameContext) -> super::View {
        mq::clear_background(super::BACKGROUND_COLOR);

        let mut next_view: Option<super::View> = None;

        egui_macroquad::ui(|egui_ctx| {
            egui::CentralPanel::default()
                .frame(egui::Frame::none())
                .show(egui_ctx, |ui| {
                    let ui_screen_width = mq::screen_width() / egui_ctx.zoom_factor();
                    let ui_screen_height = mq::screen_height() / egui_ctx.zoom_factor();

                    let time_size = 30.0;

                    let box_width = 250.0;
                    let box_height = (25.0 + ui.spacing().item_spacing.y) * 3.0 + time_size;

                    let box_x = ui_screen_width / 2.0 - box_width / 2.0;
                    let box_y = ui_screen_height / 2.0 - box_height / 2.0;

                    ui.allocate_ui_at_rect(
                        egui::Rect {
                            min: egui::Pos2::new(box_x, box_y),
                            max: egui::Pos2::new(box_x + box_width, box_y + box_height),
                        },
                        |ui| {
                            ui.vertical_centered(|ui| {
                                ui.label(
                                    egui::RichText::new(format!("{:.2}", self.time))
                                        .size(time_size),
                                );

                                let best_text = match self.previous_best_time {
                                    None | Some(None) => None,
                                    Some(Some(previous_best_time)) => {
                                        if previous_best_time > self.time {
                                            Some(format!(
                                                "New record! Your previous best: {:.2}",
                                                previous_best_time
                                            ))
                                        } else {
                                            Some(format!("Your best: {:.2}", previous_best_time))
                                        }
                                    }
                                };

                                match best_text {
                                    None => {
                                        ui.label("");
                                    }
                                    Some(text) => {
                                        ui.label(text);
                                    }
                                }

                                if ui.button("Play Again").clicked() {
                                    next_view = Some(PracticeHandView::new(self.spec).into());
                                } else if ui.button("Main Menu").clicked() {
                                    next_view = Some(super::MainMenuView::init(ctx).into());
                                }
                            });
                        },
                    );
                });
        });

        egui_macroquad::draw();

        next_view.unwrap_or_else(|| self.into())
    }
}

pub struct PracticeSetupView {
    spec: PracticeSpec,
}

impl Default for PracticeSetupView {
    fn default() -> Self {
        Self {
            spec: PracticeSpec::Distribute,
        }
    }
}

impl super::ViewImpl for PracticeSetupView {
    fn tick(mut self, _ctx: &mut super::GameContext) -> super::View {
        let mut do_start = false;

        mq::clear_background(super::BACKGROUND_COLOR);

        egui_macroquad::ui(|egui_ctx| {
            egui::CentralPanel::default()
                .frame(egui::Frame::none())
                .show(egui_ctx, |ui| {
                    let ui_screen_width = mq::screen_width() / egui_ctx.zoom_factor();
                    let ui_screen_height = mq::screen_height() / egui_ctx.zoom_factor();

                    let box_width = 250.0;
                    let box_height = 25.0 + (25.0 + ui.spacing().item_spacing.y) * 1.0;

                    let box_x = ui_screen_width / 2.0 - box_width / 2.0;
                    let box_y = ui_screen_height / 2.0 - box_height / 2.0;

                    ui.allocate_ui_at_rect(
                        egui::Rect {
                            min: egui::Pos2::new(box_x, box_y),
                            max: egui::Pos2::new(box_x + box_width, box_y + box_height),
                        },
                        |ui| {
                            egui::ComboBox::from_label("Scenario")
                                .selected_text(self.spec.to_string())
                                .show_ui(ui, |ui| {
                                    for spec in PracticeSpec::iter() {
                                        ui.selectable_value(&mut self.spec, spec, spec.to_string());
                                    }
                                });

                            ui.vertical_centered(|ui| {
                                if ui.button("Start").clicked() {
                                    do_start = true;
                                }
                            });
                        },
                    );
                });
        });

        egui_macroquad::draw();

        if do_start {
            PracticeHandView::new(self.spec).into()
        } else {
            self.into()
        }
    }
}

fn lake_stack() -> ni_ty::Stack {
    ni_ty::Stack::new(ni_ty::Ordering::SingleSuitUp, true)
}
