use super::ingame_hand_common;
use macroquad::prelude as mq;
use nertsio_types as ni_ty;
use std::collections::HashMap;

const START_ANIMATION_SPEED: f32 = 3000.0;
const START_TIME: std::time::Duration = std::time::Duration::from_secs(3);

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
}

pub struct PracticeHandView {
    spec: PracticeSpec,
    hand: ni_ty::HandState,
    my_held_state: Option<crate::HeldState>,
    start_animation_progress: f32,
    started_at: web_time::Instant,
}

impl PracticeHandView {
    pub fn new(spec: PracticeSpec) -> Self {
        Self {
            hand: spec.gen_hand(),
            spec,
            my_held_state: None,
            start_animation_progress: 0.0,
            started_at: web_time::Instant::now(),
        }
    }
}

impl super::ViewImpl for PracticeHandView {
    fn tick(mut self, ctx: &mut super::GameContext) -> super::View {
        let hand = &mut self.hand;
        let my_held_state = &mut self.my_held_state;

        let metrics = ingame_hand_common::hand_metrics(&hand);

        let real_screen_size = (mq::screen_width(), mq::screen_height());
        let screen_size = ingame_hand_common::screen_size_for_hand(real_screen_size, &metrics);
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

        let started = hand.started;

        mq::clear_background(super::BACKGROUND_COLOR);

        let location = metrics.player_loc(0);
        let position = mq::Vec2::from(location.pos()) + mq::Vec2::from(screen_center);

        let mouse_pos = mq::mouse_position();
        let mouse_pos = mq::Vec2::new(
            mouse_pos.0 * screen_size.0 / real_screen_size.0,
            mouse_pos.1 * screen_size.1 / real_screen_size.1,
        );

        {
            let mut settings_lock = ctx.settings_mutex.lock().unwrap();
            let settings = &mut *settings_lock;

            if let Some(action) = ingame_hand_common::handle_input(
                ctx,
                settings,
                &metrics,
                screen_center.into(),
                0,
                &hand,
                my_held_state,
                mouse_pos,
            ) {
                hand.apply(Some(0), action)
                    .expect("Failed to apply player action");
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
            ingame_hand_common::draw_held_state(ctx, &hand, 0, held_info, mouse_pos);
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

        if !started {
            self.start_animation_progress += mq::get_frame_time() * START_ANIMATION_SPEED;
        }

        self.into()
    }
}

fn lake_stack() -> ni_ty::Stack {
    ni_ty::Stack::new(ni_ty::Ordering::SingleSuitUp, true)
}
