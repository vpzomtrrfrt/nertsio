use super::ingame_hand_common;
use macroquad::prelude as mq;
use nertsio_types as ni_ty;

pub enum PracticeSpec {
    Invert,
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
                vec![ni_ty::Stack::new(ni_ty::Ordering::SingleSuitUp, true)],
            ),
        }
    }
}

pub struct PracticeHandView {
    spec: PracticeSpec,
    hand: ni_ty::HandState,
    my_held_state: Option<crate::HeldState>,
}

impl PracticeHandView {
    pub fn new(spec: PracticeSpec) -> Self {
        Self {
            hand: spec.gen_hand(),
            spec,
            my_held_state: None,
        }
    }
}

impl super::ViewImpl for PracticeHandView {
    fn tick(self, ctx: &mut super::GameContext) -> super::View {
        let mut hand = self.hand;
        let mut my_held_state = self.my_held_state;

        let metrics = ingame_hand_common::hand_metrics(&hand);

        let real_screen_size = (mq::screen_width(), mq::screen_height());
        let screen_size = ingame_hand_common::screen_size_for_hand(real_screen_size, &metrics);
        let scale = real_screen_size.0 / screen_size.0;

        let screen_center = (screen_size.0 / 2.0, screen_size.1 / 2.0);

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
                &mut my_held_state,
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
            true,
            1.0,
        );

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

        if let Some(held_info) = held_info {
            ingame_hand_common::draw_held_state(ctx, &hand, 0, held_info, mouse_pos);
        }

        Self {
            my_held_state,
            hand,
            ..self
        }
        .into()
    }
}
