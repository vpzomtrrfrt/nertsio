use super::ingame_hand_common;
use crate::storage::Storage;
use macroquad::logging as log;
use macroquad::prelude as mq;
use nertsio_types as ni_ty;
use nertsio_ui_metrics as metrics;
use rand::Rng;
use std::collections::{HashMap, HashSet};
use strum::IntoEnumIterator;

const START_ANIMATION_SPEED: f32 = 5000.0;
const START_TIME: std::time::Duration = std::time::Duration::from_secs(3);

const FEEDER_SPEED: f32 = 65.0;

#[derive(Clone, Copy, PartialEq, Debug, strum_macros::EnumIter, strum_macros::Display)]
pub enum PracticeSpec {
    Invert,
    Distribute,
    Stack,
    Flip,
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
            PracticeSpec::Stack => ni_ty::HandState::raw(
                vec![ni_ty::HandPlayerState::raw(
                    0,
                    ni_ty::Stack::from_list_unordered({
                        let mut stacks: Vec<_> = ni_ty::Suit::iter()
                            .map(|suit| {
                                vec![ni_ty::CardInstance::new(
                                    ni_ty::Card::new(suit, ni_ty::Rank::KING),
                                    0,
                                )]
                            })
                            .collect();

                        let stacks_count = 5;

                        let mut rng = rand::thread_rng();

                        rand::seq::SliceRandom::shuffle(&mut stacks[..], &mut rng);

                        let mut rank = ni_ty::Rank::KING.decrement().unwrap();

                        loop {
                            let mut next_red: bool = rng.gen();
                            let mut next_black: bool = rng.gen();

                            for stack in &mut stacks {
                                let last = stack.last().unwrap();
                                let new_suit = match last.card.suit.color() {
                                    ni_ty::Color::Red => {
                                        next_black = !next_black;

                                        if next_black {
                                            ni_ty::Suit::Clubs
                                        } else {
                                            ni_ty::Suit::Spades
                                        }
                                    }
                                    ni_ty::Color::Black => {
                                        next_red = !next_red;

                                        if next_red {
                                            ni_ty::Suit::Diamonds
                                        } else {
                                            ni_ty::Suit::Hearts
                                        }
                                    }
                                };

                                stack.push(ni_ty::CardInstance::new(
                                    ni_ty::Card::new(new_suit, rank),
                                    0,
                                ));
                            }

                            match rank.decrement() {
                                Some(value) => {
                                    rank = value;
                                }
                                None => {
                                    break;
                                }
                            }
                        }

                        let mut nerts_cards = Vec::new();

                        while !stacks.is_empty() {
                            let idx = rng.gen_range(0..stacks.len());

                            let current_stacks_count = stacks.len();

                            let stack = stacks.get_mut(idx).unwrap();
                            if current_stacks_count < stacks_count && stack.len() > 1 {
                                let split_idx = rng.gen_range(0..stack.len());

                                let after = stack.split_off(split_idx + 1);

                                nerts_cards.push(stack.pop().unwrap());

                                if stack.is_empty() {
                                    stacks.swap_remove(idx);
                                }

                                if !after.is_empty() {
                                    stacks.push(after);
                                }
                            } else {
                                nerts_cards.push(stack.pop().unwrap());
                                if stack.is_empty() {
                                    stacks.swap_remove(idx);
                                }
                            }
                        }

                        nerts_cards
                    }),
                    ni_ty::Stack::new(ni_ty::Ordering::Any, false),
                    ni_ty::Stack::new(ni_ty::Ordering::Any, false),
                    (0..5)
                        .map(|_| ni_ty::Stack::new(ni_ty::Ordering::AlternatingDown, false))
                        .collect(),
                )],
                vec![],
            ),
            PracticeSpec::Flip => {
                let mut rng = rand::thread_rng();

                ni_ty::HandState::raw(
                    vec![
                        ni_ty::HandPlayerState::raw(
                            0,
                            ni_ty::Stack::new(ni_ty::Ordering::Any, false),
                            ni_ty::Stack::from_list_unordered({
                                let mut result: Vec<_> = ni_ty::gen_player_deck(0).collect();
                                rand::seq::SliceRandom::shuffle(&mut result[..], &mut rng);
                                result
                            }),
                            ni_ty::Stack::new(ni_ty::Ordering::Any, false),
                            vec![],
                        ),
                        ni_ty::HandPlayerState::raw(
                            1,
                            ni_ty::Stack::new(ni_ty::Ordering::Any, false),
                            ni_ty::Stack::from_list_unordered({
                                let mut result: Vec<_> = ni_ty::Suit::iter()
                                    .map(|suit| {
                                        ni_ty::CardInstance::new(
                                            ni_ty::Card::new(suit, ni_ty::Rank::ACE),
                                            1,
                                        )
                                    })
                                    .collect();
                                rand::seq::SliceRandom::shuffle(&mut result[..], &mut rng);
                                result
                            }),
                            ni_ty::Stack::new(ni_ty::Ordering::Any, false),
                            vec![],
                        ),
                    ],
                    (0..(2 * 4)).map(|_| lake_stack()).collect(),
                )
            }
        }
    }

    pub fn is_done(&self, hand: &ni_ty::HandState) -> bool {
        match self {
            PracticeSpec::Invert | PracticeSpec::Distribute | PracticeSpec::Stack => {
                hand.players()[0].nerts_stack().is_empty()
            }
            PracticeSpec::Flip => {
                let player = &hand.players()[0];

                player.stock_stack().is_empty() && player.waste_stack().is_empty()
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum FeederPlan {
    Summon,
    Retract {
        from: ni_ty::StackLocation,
        count: u8,
    },
    HandAction(ni_ty::HandAction),
}

enum FeederPlanStep {
    Move(nertsio_ui_metrics::Rect),
    Action(FeederPlan),
    None,
}

struct FeederState {
    mouse: ni_ty::MouseState,
    plan: Option<FeederPlan>,
}

pub struct PracticeHandView {
    spec: PracticeSpec,
    hand: ni_ty::HandState,
    my_held_state: Option<crate::HeldState>,
    start_animation_progress: f32,
    started_at: web_time::Instant,
    time: f32,
    feeder_state: FeederState,
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
            feeder_state: FeederState {
                mouse: ni_ty::MouseState {
                    position: (0.0, 0.0),
                    held: None,
                },
                plan: None,
            },
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
                PracticeSpec::Distribute | PracticeSpec::Stack => {
                    usize::from(ni_ty::Rank::COUNT) * 4
                }
                PracticeSpec::Flip => 0,
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
                    ctx.play_sound_for_action(action, true);

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

        let my_location = metrics.player_loc(0);

        ingame_hand_common::draw_player_stacks(
            ctx,
            &hand.players()[0],
            &held_info,
            &metrics,
            my_location,
            screen_center.into(),
            started,
            self.start_animation_progress,
        );

        if let Some(feeder_player) = hand.players().get(1) {
            let location = metrics.player_loc(1);

            if location.inverted != my_location.inverted {
                mq::set_camera(&inverted_camera);
            }

            ingame_hand_common::draw_player_stacks(
                ctx,
                feeder_player,
                &self.feeder_state.mouse.held,
                &metrics,
                location,
                screen_center.into(),
                started,
                self.start_animation_progress,
            );
        }

        if my_location.inverted {
            mq::set_camera(&inverted_camera);
        } else {
            mq::set_camera(&normal_camera);
        }

        for (i, stack) in hand.lake_stacks().iter().enumerate() {
            let loc = ni_ty::StackLocation::Lake(i as u16);
            let pos = mq::Vec2::from(metrics.stack_pos(loc)) + mq::Vec2::from(screen_center);

            let cards = stack.cards();
            let cards = match self.feeder_state.mouse.held {
                Some(ni_ty::HeldInfo {
                    src: ni_ty::StackLocation::Lake(stack_idx),
                    count,
                    ..
                }) => {
                    if i == usize::from(stack_idx) && usize::from(count) <= cards.len() {
                        &cards[..(cards.len() - usize::from(count))]
                    } else {
                        cards
                    }
                }
                _ => cards,
            };

            match cards.last() {
                None => {
                    ctx.draw_placeholder(pos[0], pos[1]);
                }
                Some(card) => {
                    ctx.draw_card(card.card, pos[0], pos[1]);
                }
            }
        }

        if let Some(feeder_player) = hand.players().get(1) {
            let location = metrics.player_loc(1);

            if location.inverted != my_location.inverted {
                mq::set_camera(&inverted_camera);
            } else {
                mq::set_camera(&normal_camera);
            }

            if let Some(held) = self.feeder_state.mouse.held {
                ingame_hand_common::draw_held_state(
                    ctx,
                    &hand,
                    1,
                    held,
                    mq::Vec2::from(screen_center)
                        + mq::Vec2::from(self.feeder_state.mouse.position),
                );
            }

            ctx.draw_cursor(
                screen_center.0 + self.feeder_state.mouse.position.0 - 1.0,
                screen_center.1 + self.feeder_state.mouse.position.1 - 1.0,
                feeder_player.player_id(),
            );
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

        match self.spec {
            PracticeSpec::Flip => {
                let stock_pos = metrics.stack_pos(ni_ty::StackLocation::Player(
                    1,
                    ni_ty::PlayerStackLocation::Stock,
                ));

                if self.feeder_state.plan.is_none() {
                    if !(mq::Rect {
                        x: stock_pos.0,
                        y: stock_pos.1,
                        w: metrics::CARD_WIDTH,
                        h: metrics::CARD_HEIGHT,
                    })
                    .contains(self.feeder_state.mouse.position.into())
                    {
                        log::debug!("setting plan to summon");
                        self.feeder_state.plan = Some(FeederPlan::Summon);
                    } else if started {
                        let my_player = &hand.players()[0];

                        let my_available_cards: HashSet<_> = (
                            // cards on the next pass
                            my_player
                                .waste_stack()
                                .cards()
                                .iter()
                                .chain(my_player.stock_stack().cards().iter().rev())
                                .skip(2)
                                .step_by(3)
                        )
                        .chain(
                            // remaining cards on this pass
                            my_player
                                .stock_stack()
                                .cards()
                                .iter()
                                .rev()
                                .skip(2)
                                .step_by(3),
                        )
                        .chain(
                            // currently available waste card
                            my_player.waste_stack().last(),
                        )
                        .chain(
                            // last card in stock
                            my_player.stock_stack().first(),
                        )
                        .map(|x| x.card)
                        .collect();

                        let mut playable_count = my_available_cards
                            .iter()
                            .filter(|x| x.rank == ni_ty::Rank::ACE)
                            .count();
                        for stack in hand.lake_stacks() {
                            if let Some(last) = stack.last() {
                                if let Some(next) = last.card.rank.increment() {
                                    if my_available_cards
                                        .contains(&ni_ty::Card::new(last.card.suit, next))
                                    {
                                        playable_count += 1;
                                    }
                                }
                            }
                        }

                        log::debug!(
                            "feeder: {}/{} playable",
                            playable_count,
                            my_available_cards.len()
                        );

                        if playable_count < my_available_cards.len() / 3 || playable_count < 2 {
                            // not enough playable, feed one

                            let mut aces = hand.players()[1]
                                .stock_stack()
                                .cards()
                                .iter()
                                .filter(|x| x.card.rank == ni_ty::Rank::ACE);

                            let current_tops: HashSet<_> = hand
                                .lake_stacks()
                                .iter()
                                .filter_map(|x| x.last().map(|x| x.card))
                                .collect();

                            let has_empty = hand.lake_stacks().iter().any(|x| x.is_empty());

                            {
                                let mut state: Vec<_> = hand
                                    .lake_stacks()
                                    .iter()
                                    .enumerate()
                                    .filter_map(|(idx, x)| match x.last() {
                                        Some(x) => x.card.rank.increment().and_then(|rank| {
                                            rank.increment().map(|following_rank| {
                                                (
                                                    idx,
                                                    ni_ty::Card::new(x.card.suit, rank),
                                                    ni_ty::Card::new(x.card.suit, following_rank),
                                                )
                                            })
                                        }),
                                        None => aces.next().map(|card| {
                                            (
                                                idx,
                                                card.card,
                                                ni_ty::Card::new(
                                                    card.card.suit,
                                                    card.card.rank.increment().unwrap(),
                                                ),
                                            )
                                        }),
                                    })
                                    .filter(|(_, card, _)| !my_available_cards.contains(card))
                                    .collect();

                                let feeder_player = hand.players_mut().get_mut(1).unwrap();

                                'outer: while !state.is_empty() {
                                    log::debug!("new state: {:?}", state);

                                    for (target_idx, play_card, target_card) in &state {
                                        if my_available_cards.contains(target_card) {
                                            // remove used aces
                                            let stock_stack = feeder_player.stock_stack_mut();
                                            if let Some(idx) = stock_stack
                                                .cards()
                                                .iter()
                                                .position(|x| x.card == *play_card)
                                            {
                                                stock_stack.cards_mut().swap_remove(idx);
                                            }

                                            feeder_player
                                                .waste_stack_mut()
                                                .try_add(ni_ty::CardInstance::new(*play_card, 1))
                                                .unwrap();

                                            log::debug!(
                                                "summoned a card {:?} {:?}",
                                                play_card,
                                                target_card
                                            );

                                            self.feeder_state.plan = Some(FeederPlan::HandAction(
                                                ni_ty::HandAction::Move {
                                                    from: ni_ty::StackLocation::Player(
                                                        1,
                                                        ni_ty::PlayerStackLocation::Waste,
                                                    ),
                                                    count: 1,
                                                    to: ni_ty::StackLocation::Lake(
                                                        *target_idx as u16,
                                                    ),
                                                },
                                            ));

                                            break 'outer;
                                        }
                                    }

                                    state = state
                                        .into_iter()
                                        .filter_map(|(idx, play_card, target_card)| {
                                            // don't try to bring up a duplicate stack
                                            if match target_card.rank.decrement() {
                                                Some(next_rank) => current_tops.contains(
                                                    &ni_ty::Card::new(target_card.suit, next_rank),
                                                ),
                                                None => false,
                                            } {
                                                None
                                            } else {
                                                target_card.rank.increment().map(|rank| {
                                                    (
                                                        idx,
                                                        play_card,
                                                        ni_ty::Card {
                                                            suit: target_card.suit,
                                                            rank,
                                                        },
                                                    )
                                                })
                                            }
                                        })
                                        .collect();
                                }
                            }

                            if self.feeder_state.plan.is_none() {
                                // No plan to add cards, maybe remove some

                                let mut state: Vec<_> = hand
                                    .lake_stacks()
                                    .iter()
                                    .enumerate()
                                    .filter_map(|(idx, stack)| {
                                        stack.last().map(|card| (idx, 1, card.card))
                                    })
                                    .collect();

                                'outer: while !state.is_empty() {
                                    for (target_idx, take_count, target_card) in &state {
                                        if my_available_cards.contains(target_card) {
                                            // Removing these would allow the player to play a card

                                            // Make sure the current state isn't *also* needed for
                                            // an available card
                                            if let Some(next_rank) = target_card.rank.increment() {
                                                if my_available_cards.contains(&ni_ty::Card::new(
                                                    target_card.suit,
                                                    next_rank,
                                                )) {
                                                    continue;
                                                }
                                            }

                                            // Make sure the resulting stack isn't a duplicate
                                            if match target_card.rank.decrement() {
                                                Some(prev_rank) => current_tops.contains(
                                                    &ni_ty::Card::new(target_card.suit, prev_rank),
                                                ),
                                                None => has_empty,
                                            } {
                                                continue;
                                            }

                                            self.feeder_state.plan = Some(FeederPlan::Retract {
                                                from: ni_ty::StackLocation::Lake(
                                                    *target_idx as u16,
                                                ),
                                                count: *take_count,
                                            });

                                            break 'outer;
                                        }
                                    }

                                    state = state
                                        .into_iter()
                                        .filter_map(|(target_idx, take_count, target_card)| {
                                            target_card.rank.decrement().map(|rank| {
                                                (
                                                    target_idx,
                                                    take_count + 1,
                                                    ni_ty::Card::new(target_card.suit, rank),
                                                )
                                            })
                                        })
                                        .collect();
                                }
                            }

                            match self.feeder_state.plan {
                                Some(plan) => {
                                    log::debug!("Got a plan: {:?}", plan);
                                }
                                None => {
                                    log::debug!("still no plan");
                                }
                            }
                        }
                    }
                }
            }
            _ => {}
        }

        if hand.players().len() > 1 {
            let feeder_loc = metrics.player_loc(1);

            if let Some(plan) = self.feeder_state.plan {
                let get_dest_for_stack = |loc, take_count| {
                    metrics.get_dest_for_stack(&hand, loc, take_count, feeder_loc.inverted)
                };

                let result = match plan {
                    FeederPlan::Retract { from, count } => {
                        if self.feeder_state.mouse.held.is_some() {
                            let dest = get_dest_for_stack(
                                ni_ty::StackLocation::Player(1, ni_ty::PlayerStackLocation::Stock),
                                0,
                            );

                            if dest.contains(self.feeder_state.mouse.position) {
                                FeederPlanStep::Action(plan)
                            } else {
                                FeederPlanStep::Move(dest)
                            }
                        } else {
                            let dest = get_dest_for_stack(from, count.into());

                            if dest.contains(self.feeder_state.mouse.position) {
                                let from_stack = hand.stack_at(from).unwrap();
                                if from_stack.len() >= count.into() {
                                    self.feeder_state.mouse.held = Some(ni_ty::HeldInfo {
                                        src: from,
                                        count,
                                        offset: (
                                            self.feeder_state.mouse.position.0 - dest.x,
                                            self.feeder_state.mouse.position.1 - dest.y,
                                        ),
                                        top_card: from_stack.cards()
                                            [from_stack.len() - usize::from(count)]
                                        .card,
                                    });

                                    FeederPlanStep::Move(get_dest_for_stack(
                                        ni_ty::StackLocation::Player(
                                            1,
                                            ni_ty::PlayerStackLocation::Stock,
                                        ),
                                        0,
                                    ))
                                } else {
                                    log::debug!("clearing plan because it's impossible");
                                    self.feeder_state.plan = None;
                                    FeederPlanStep::None
                                }
                            } else {
                                FeederPlanStep::Move(dest)
                            }
                        }
                    }
                    FeederPlan::Summon
                    | FeederPlan::HandAction(
                        ni_ty::HandAction::FlipStock | ni_ty::HandAction::ReturnStock,
                    ) => {
                        let dest = get_dest_for_stack(
                            ni_ty::StackLocation::Player(1, ni_ty::PlayerStackLocation::Stock),
                            0,
                        );

                        if dest.contains(self.feeder_state.mouse.position) {
                            FeederPlanStep::Action(plan)
                        } else {
                            FeederPlanStep::Move(dest)
                        }
                    }
                    FeederPlan::HandAction(ni_ty::HandAction::Move { from, count, to }) => {
                        if self.feeder_state.mouse.held.is_some() {
                            log::debug!("there is a held");
                            let dest = get_dest_for_stack(to, 0);

                            if dest.contains(self.feeder_state.mouse.position) {
                                log::debug!("got to destination");
                                FeederPlanStep::Action(plan)
                            } else {
                                FeederPlanStep::Move(dest)
                            }
                        } else {
                            let dest = get_dest_for_stack(from, count.into());

                            if dest.contains(self.feeder_state.mouse.position) {
                                log::debug!("got to source");
                                let from_stack = hand.stack_at(from).unwrap();
                                if from_stack.len() >= count.into() {
                                    self.feeder_state.mouse.held = Some(ni_ty::HeldInfo {
                                        src: from,
                                        count,
                                        offset: (
                                            self.feeder_state.mouse.position.0 - dest.x,
                                            self.feeder_state.mouse.position.1 - dest.y,
                                        ),
                                        top_card: from_stack.cards()
                                            [from_stack.len() - usize::from(count)]
                                        .card,
                                    });

                                    FeederPlanStep::Move(get_dest_for_stack(to, 0))
                                } else {
                                    log::debug!("clearing plan because it's impossible");
                                    self.feeder_state.plan = None;
                                    FeederPlanStep::None
                                }
                            } else {
                                FeederPlanStep::Move(dest)
                            }
                        }
                    }
                    FeederPlan::HandAction(_) => unimplemented!(),
                };

                match result {
                    FeederPlanStep::Move(target_rect) => {
                        let target = target_rect.center();

                        let dist = ((self.feeder_state.mouse.position.0 - target.0).powf(2.0)
                            + (self.feeder_state.mouse.position.1 - target.1).powf(2.0))
                        .sqrt();

                        let speed = FEEDER_SPEED * mq::get_frame_time() * 20.0;

                        if dist <= speed {
                            self.feeder_state.mouse.position = target;
                        } else {
                            self.feeder_state.mouse.position = (
                                self.feeder_state.mouse.position.0
                                    + (target.0 - self.feeder_state.mouse.position.0) / dist
                                        * speed,
                                self.feeder_state.mouse.position.1
                                    + (target.1 - self.feeder_state.mouse.position.1) / dist
                                        * speed,
                            );
                        }
                    }
                    FeederPlanStep::Action(action) => {
                        log::debug!("clearing plan because it's done");
                        self.feeder_state.plan = None;

                        match action {
                            FeederPlan::Summon => {
                                // Summon isn't a real action, and will be handled elsewhere.
                                // Do nothing.
                            }
                            FeederPlan::Retract { from, count } => {
                                let stack = hand.stack_at_mut(from).unwrap();
                                if let Some(cards) = stack.pop_many(count.into()) {
                                    let target = hand
                                        .stack_at_mut(ni_ty::StackLocation::Player(
                                            1,
                                            ni_ty::PlayerStackLocation::Stock,
                                        ))
                                        .unwrap();
                                    for card in cards {
                                        target.try_add(card).unwrap();
                                    }
                                }
                                self.feeder_state.mouse.held = None;
                            }
                            FeederPlan::HandAction(action) => {
                                // Attempt to apply the action
                                let _ = hand.apply(Some(1), action);

                                // Regardless of whether it succeeded, clear out held info
                                self.feeder_state.mouse.held = None;
                            }
                        }
                    }
                    FeederPlanStep::None => {}
                }
            }
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
    fn tick(mut self, ctx: &mut super::GameContext) -> super::View {
        let mut do_start = false;
        let mut do_leave = false;

        mq::clear_background(super::BACKGROUND_COLOR);

        egui_macroquad::ui(|egui_ctx| {
            egui::CentralPanel::default()
                .frame(egui::Frame::none().inner_margin(egui::Margin::same(super::SCREEN_MARGIN)))
                .show(egui_ctx, |ui| {
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Min), |ui| {
                        if ui.button("Leave").clicked() {
                            do_leave = true;
                        }
                    });

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

        do_leave = do_leave || mq::is_key_pressed(mq::KeyCode::Escape);

        if do_leave {
            super::MainMenuView::init(ctx).into()
        } else if do_start {
            PracticeHandView::new(self.spec).into()
        } else {
            self.into()
        }
    }
}

fn lake_stack() -> ni_ty::Stack {
    ni_ty::Stack::new(ni_ty::Ordering::SingleSuitUp, true)
}
