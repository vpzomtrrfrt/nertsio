use crate::ConnectionMessage;
use macroquad::logging as log;
use macroquad::prelude as mq;
use nertsio_types as ni_ty;
use nertsio_ui_metrics as metrics;
use std::borrow::Cow;

const NERTS_OVERLAY_COLOR: mq::Color = mq::Color::new(1.0, 1.0, 1.0, 0.4);
const NERTS_TEXT_COLOR: mq::Color = mq::Color::new(0.0, 0.0, 1.0, 1.0);

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

                let metrics = metrics::HandMetrics::new(
                    real_hand_state.players().len(),
                    real_hand_state.players()[0].tableau_stacks().len(),
                    real_hand_state.lake_stacks().len(),
                );

                let needed_screen_width = metrics.needed_screen_width();
                let needed_screen_height = metrics.needed_screen_height() + super::SCREEN_MARGIN;

                let real_screen_size = (mq::screen_width(), mq::screen_height());
                let screen_size = {
                    let mut factor = (real_screen_size.0 / needed_screen_width)
                        .min(real_screen_size.1 / needed_screen_height);

                    println!("factor = {}", factor);

                    if factor > 1.0 {
                        // round down to nearest 0.5
                        factor = (factor * 2.0).floor() / 2.0;
                    }

                    (real_screen_size.0 / factor, real_screen_size.1 / factor)
                };

                let scale = real_screen_size.0 / screen_size.0;

                let camera_rect =
                    mq::Rect::new(0.0, screen_size.1, screen_size.0, -screen_size.1).into();

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

                let (pred_hand_state, self_inverted) = if let Some(my_player_idx) =
                    self.my_player_idx
                {
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
                        let player_state = &pred_hand_state.players()[my_player_idx];

                        let mouse_pressed = mq::is_mouse_button_pressed(mq::MouseButton::Left);

                        let mut settings_lock = ctx.settings_mutex.lock().unwrap();
                        let settings = &mut *settings_lock;

                        if mouse_pressed
                            || (mq::is_mouse_button_released(mq::MouseButton::Left)
                                && settings.drag)
                        {
                            let nerts_stack_pos =
                                mq::Vec2::from(metrics.player_stack_pos(
                                    ni_ty::PlayerStackLocation::Nerts,
                                    my_location,
                                )) + mq::Vec2::from(screen_center);
                            let stock_stack_pos =
                                mq::Vec2::from(metrics.player_stack_pos(
                                    ni_ty::PlayerStackLocation::Stock,
                                    my_location,
                                )) + mq::Vec2::from(screen_center);
                            let waste_stack_pos =
                                mq::Vec2::from(metrics.player_stack_pos(
                                    ni_ty::PlayerStackLocation::Waste,
                                    my_location,
                                )) + mq::Vec2::from(screen_center);

                            if mq::Rect::new(
                                stock_stack_pos[0],
                                stock_stack_pos[1],
                                metrics::CARD_WIDTH,
                                metrics::CARD_HEIGHT,
                            )
                            .contains(mouse_pos)
                            {
                                if mouse_pressed {
                                    let action = if player_state.stock_stack().len() > 0 {
                                        ni_ty::HandAction::FlipStock
                                    } else {
                                        ni_ty::HandAction::ReturnStock
                                    };

                                    if pred_hand_state
                                        .apply(Some(my_player_idx_u8), action)
                                        .is_ok()
                                    {
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

                                        hand_extra.my_held_state = None;
                                    }
                                }
                            } else {
                                let found = if player_state.nerts_stack().len() > 0
                                    && mq::Rect::new(
                                        nerts_stack_pos[0]
                                            + ((player_state.nerts_stack().len() - 1) as f32)
                                                * metrics::HORIZONTAL_STACK_SPACING,
                                        nerts_stack_pos[1],
                                        metrics::CARD_WIDTH,
                                        metrics::CARD_HEIGHT,
                                    )
                                    .contains(mouse_pos)
                                {
                                    Some((
                                        ni_ty::StackLocation::Player(
                                            my_player_idx_u8,
                                            ni_ty::PlayerStackLocation::Nerts,
                                        ),
                                        1,
                                        mouse_pos
                                            - mq::Vec2::new(
                                                nerts_stack_pos[0]
                                                    + ((player_state.nerts_stack().len() - 1)
                                                        as f32)
                                                        * metrics::HORIZONTAL_STACK_SPACING,
                                                nerts_stack_pos[1],
                                            ),
                                    ))
                                } else if mq::Rect::new(
                                    screen_center.0 + metrics.lake_start_x(),
                                    screen_center.1 - metrics::CARD_HEIGHT / 2.0,
                                    metrics.lake_width(),
                                    metrics::CARD_HEIGHT,
                                )
                                .contains(mouse_pos)
                                {
                                    let stack_idx_for_me = ((mouse_pos[0]
                                        - (screen_center.0 + metrics.lake_start_x()))
                                        / (metrics::CARD_WIDTH + metrics::LAKE_SPACING))
                                        as u16;

                                    let stack_idx = if my_location.inverted {
                                        (pred_hand_state.lake_stacks().len() as u16)
                                            - stack_idx_for_me
                                            - 1
                                    } else {
                                        stack_idx_for_me
                                    };

                                    let loc = ni_ty::StackLocation::Lake(stack_idx);
                                    let stack_pos = mq::Vec2::from(metrics.stack_pos(loc))
                                        + mq::Vec2::from(screen_center);

                                    Some((loc, 1, mouse_pos - stack_pos))
                                } else if mq::Rect::new(
                                    waste_stack_pos[0],
                                    waste_stack_pos[1],
                                    metrics::CARD_WIDTH + metrics::HORIZONTAL_STACK_SPACING * 2.0,
                                    metrics::CARD_HEIGHT,
                                )
                                .contains(mouse_pos)
                                    && player_state.waste_stack().len() > 0
                                {
                                    Some((
                                        ni_ty::StackLocation::Player(
                                            my_player_idx_u8,
                                            ni_ty::PlayerStackLocation::Waste,
                                        ),
                                        1,
                                        mouse_pos
                                            - mq::Vec2::new(
                                                waste_stack_pos[0]
                                                    + (metrics::HORIZONTAL_STACK_SPACING
                                                        * ((player_state.waste_stack().len().min(3)
                                                            - 1)
                                                            as f32)),
                                                waste_stack_pos[1],
                                            ),
                                    ))
                                } else {
                                    player_state
                                        .tableau_stacks()
                                        .iter()
                                        .enumerate()
                                        .filter_map(|(i, stack)| {
                                            let loc = ni_ty::PlayerStackLocation::Tableau(i as u8);

                                            let stack_pos = mq::Vec2::from(metrics.player_stack_pos(loc, my_location)) + mq::Vec2::from(screen_center);
                                            if mq::Rect::new(
                                                stack_pos[0],
                                                stack_pos[1],
                                                metrics::CARD_WIDTH,
                                                metrics::CARD_HEIGHT
                                                    + ((stack.len() as f32) - 1.0)
                                                        * metrics::VERTICAL_STACK_SPACING,
                                            )
                                            .contains(mouse_pos)
                                            {
                                                let loc = ni_ty::StackLocation::Player(
                                                    my_player_idx_u8,
                                                    ni_ty::PlayerStackLocation::Tableau(i as u8),
                                                );
                                                if stack.len() > 0 {
                                                    let found_idx = (((mouse_pos[1]
                                                        - stack_pos[1])
                                                        / metrics::VERTICAL_STACK_SPACING)
                                                        as usize)
                                                        .min(stack.len() - 1);

                                                    Some((
                                                        loc,
                                                        stack.len() - found_idx,
                                                        mouse_pos
                                                            - mq::Vec2::new(
                                                                stack_pos[0],
                                                                stack_pos[1]
                                                                    + ((found_idx as f32)
                                                                        * metrics::VERTICAL_STACK_SPACING),
                                                            ),
                                                    ))
                                                } else {
                                                    Some((
                                                        loc,
                                                        0,
                                                        mouse_pos - mq::Vec2::from(stack_pos),
                                                    ))
                                                }
                                            } else {
                                                None
                                            }
                                        })
                                        .next()
                                };

                                let _ = player_state;

                                log::debug!("click found {:?}", found);

                                if let Some(found) = found {
                                    match hand_extra.my_held_state {
                                        None => {
                                            if mouse_pressed {
                                                if let (
                                                    ni_ty::StackLocation::Player(_, src),
                                                    count,
                                                    offset,
                                                ) = found
                                                {
                                                    let stack =
                                                        pred_hand_state.stack_at(found.0).unwrap();
                                                    if stack.len() > 0 {
                                                        let top_card = stack.cards()
                                                            [stack.cards().len() - count]
                                                            .card;

                                                        hand_extra.my_held_state =
                                                            Some(crate::HeldState {
                                                                info: ni_ty::HeldInfo {
                                                                    src,
                                                                    count: count as u8,
                                                                    offset: (offset[0], offset[1]),
                                                                    top_card,
                                                                },
                                                                mouse_released: false,
                                                            })
                                                    }
                                                }
                                            }
                                        }
                                        Some(ref mut held) => {
                                            let src_loc = ni_ty::StackLocation::Player(
                                                my_player_idx_u8,
                                                held.info.src,
                                            );

                                            let (target_loc, ..) = found;
                                            if target_loc == src_loc {
                                                if mouse_pressed {
                                                    hand_extra.my_held_state = None;
                                                } else {
                                                    held.mouse_released = true;
                                                }
                                            } else {
                                                let success = if matches!(
                                                    target_loc,
                                                    ni_ty::StackLocation::Player(
                                                        _,
                                                        ni_ty::PlayerStackLocation::Tableau(_)
                                                    ) | ni_ty::StackLocation::Lake(_)
                                                ) {
                                                    if let Some(target_stack) =
                                                        pred_hand_state.stack_at(target_loc)
                                                    {
                                                        if let Some(src_stack) =
                                                            pred_hand_state.stack_at(src_loc)
                                                        {
                                                            let stack_cards = src_stack.cards();
                                                            let back_card = &stack_cards[stack_cards
                                                                .len()
                                                                - held.info.count as usize];

                                                            if target_stack.can_add(*back_card) {
                                                                let action =
                                                                    ni_ty::HandAction::Move {
                                                                        from: src_loc,
                                                                        count: held.info.count,
                                                                        to: target_loc,
                                                                    };

                                                                log::debug!("applying for check");
                                                                if pred_hand_state
                                                                    .apply(
                                                                        Some(my_player_idx_u8),
                                                                        action,
                                                                    )
                                                                    .is_ok()
                                                                {
                                                                    // should always be
                                                                    // true?

                                                                    hand_extra
                                                                        .pending_actions
                                                                        .push_back(action);
                                                                    ctx.game_msg_send.borrow().as_ref().unwrap().unbounded_send(ni_ty::protocol::GameMessageC2S::ApplyHandAction { action }.into()).unwrap();
                                                                }

                                                                true
                                                            } else {
                                                                log::debug!(
                                                                    "can't add {:?} to {:?}",
                                                                    back_card,
                                                                    target_stack
                                                                );

                                                                false
                                                            }
                                                        } else {
                                                            false
                                                        }
                                                    } else {
                                                        false
                                                    }
                                                } else {
                                                    false
                                                };

                                                if success
                                                    || (!held.mouse_released && settings.drag)
                                                {
                                                    hand_extra.my_held_state = None;
                                                } else if !mouse_pressed {
                                                    held.mouse_released = true;
                                                }
                                            }
                                        }
                                    }
                                } else {
                                    if let Some(held_state) = &hand_extra.my_held_state {
                                        if !held_state.mouse_released && settings.drag {
                                            hand_extra.my_held_state = None;
                                        }
                                    }
                                }
                            }
                        } else if mq::is_mouse_button_pressed(mq::MouseButton::Right)
                            || mq::is_key_pressed(mq::KeyCode::Escape)
                        {
                            hand_extra.my_held_state = None;
                        } else if mq::is_key_pressed(mq::KeyCode::Tab) {
                            let action = if player_state.stock_stack().len() > 0 {
                                ni_ty::HandAction::FlipStock
                            } else {
                                ni_ty::HandAction::ReturnStock
                            };

                            if pred_hand_state
                                .apply(Some(my_player_idx_u8), action)
                                .is_ok()
                            {
                                hand_extra.pending_actions.push_back(action);
                                ctx.game_msg_send
                                    .borrow()
                                    .as_ref()
                                    .unwrap()
                                    .unbounded_send(
                                        ni_ty::protocol::GameMessageC2S::ApplyHandAction { action }
                                            .into(),
                                    )
                                    .unwrap();

                                hand_extra.my_held_state = None;
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

                for (idx, player_state) in pred_hand_state.players().iter().enumerate() {
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
                            position[1] - 20.0,
                        )
                    } else {
                        (
                            screen_center.0 - location.x - metrics.player_hand_width() / 2.0,
                            screen_center.1 - metrics::PLAYER_Y + 20.0,
                        )
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

                    let mut animation_not_consumed = self.start_animation_progress;

                    let stock_pos = mq::Vec2::from(
                        metrics.player_stack_pos(ni_ty::PlayerStackLocation::Stock, location),
                    ) + mq::Vec2::from(screen_center);

                    if player_state.stock_stack().len() > 0 {
                        ctx.draw_back(stock_pos[0], stock_pos[1], player_state.player_id());
                    } else {
                        ctx.draw_placeholder(stock_pos[0], stock_pos[1]);
                    }

                    if player_state.nerts_stack().len() > 0 {
                        let nerts_stack_pos = mq::Vec2::from(
                            metrics.player_stack_pos(ni_ty::PlayerStackLocation::Nerts, location),
                        ) + mq::Vec2::from(screen_center);
                        let card = player_state.nerts_stack().last().unwrap();

                        if started {
                            for i in 0..(player_state.nerts_stack().len() - 1) {
                                ctx.draw_back(
                                    nerts_stack_pos[0]
                                        + (i as f32) * metrics::HORIZONTAL_STACK_SPACING,
                                    nerts_stack_pos[1],
                                    player_state.player_id(),
                                );
                            }

                            if !matches!(
                                held_info,
                                Some(ni_ty::HeldInfo {
                                    src: ni_ty::PlayerStackLocation::Nerts,
                                    ..
                                })
                            ) {
                                ctx.draw_card(
                                    card.card,
                                    nerts_stack_pos[0]
                                        + ((player_state.nerts_stack().len() - 1) as f32)
                                            * metrics::HORIZONTAL_STACK_SPACING,
                                    nerts_stack_pos[1],
                                );
                            }
                        } else {
                            for i in 0..player_state.nerts_stack().len() {
                                let target_pos = mq::Vec2::new(
                                    nerts_stack_pos[0]
                                        + (i as f32) * metrics::HORIZONTAL_STACK_SPACING,
                                    nerts_stack_pos[1],
                                );

                                let dist = stock_pos.distance(target_pos);

                                animation_not_consumed -= dist;

                                if animation_not_consumed < 0.0 {
                                    log::debug!(
                                        "portion = {}",
                                        1.0 - animation_not_consumed / dist
                                    );
                                    let pos = stock_pos
                                        .lerp(target_pos, 1.0 + animation_not_consumed / dist);
                                    ctx.draw_back(pos.x, pos.y, player_state.player_id());
                                    break;
                                } else {
                                    ctx.draw_back(
                                        target_pos.x,
                                        target_pos.y,
                                        player_state.player_id(),
                                    );
                                }
                            }
                        }
                    }

                    for (i, stack) in player_state.tableau_stacks().iter().enumerate() {
                        let cards = stack.cards();
                        let cards = if let Some(ni_ty::HeldInfo {
                            src: ni_ty::PlayerStackLocation::Tableau(stack_idx),
                            count,
                            ..
                        }) = held_info
                        {
                            if i == (stack_idx as usize) {
                                if count as usize <= cards.len() {
                                    &cards[..(cards.len() - count as usize)]
                                } else {
                                    cards
                                }
                            } else {
                                cards
                            }
                        } else {
                            cards
                        };

                        let loc = ni_ty::PlayerStackLocation::Tableau(i as u8);
                        let target_pos = mq::Vec2::from(metrics.player_stack_pos(loc, location))
                            + mq::Vec2::from(screen_center);

                        if started {
                            ctx.draw_vertical_stack_cards(cards, target_pos[0], target_pos[1]);
                        } else {
                            if animation_not_consumed > 0.0 {
                                let dist = stock_pos.distance(target_pos);

                                animation_not_consumed -= dist;

                                if animation_not_consumed < 0.0 {
                                    let pos = stock_pos
                                        .lerp(target_pos, 1.0 + animation_not_consumed / dist);

                                    ctx.draw_back(pos[0], pos[1], player_state.player_id());
                                } else {
                                    ctx.draw_back(
                                        target_pos[0],
                                        target_pos[1],
                                        player_state.player_id(),
                                    );
                                }
                            }
                        }
                    }

                    let waste_cards = player_state.waste_stack().cards();
                    let waste_cards = if waste_cards.len() > 3 {
                        &waste_cards[(waste_cards.len() - 3)..]
                    } else {
                        waste_cards
                    };
                    let waste_cards = if let Some(ni_ty::HeldInfo {
                        src: ni_ty::PlayerStackLocation::Waste,
                        count,
                        ..
                    }) = held_info
                    {
                        if count as usize <= waste_cards.len() {
                            &waste_cards[..(waste_cards.len() - count as usize)]
                        } else {
                            waste_cards
                        }
                    } else {
                        waste_cards
                    };

                    if waste_cards.len() > 0 {
                        let waste_pos = mq::Vec2::from(
                            metrics.player_stack_pos(ni_ty::PlayerStackLocation::Waste, location),
                        ) + mq::Vec2::from(screen_center);

                        ctx.draw_horizontal_stack_cards(waste_cards, waste_pos[0], waste_pos[1]);
                    }

                    if hand_extra.stalled {
                        ctx.draw_text(
                            "Shuffling soon if game remains stalled...",
                            stock_pos[0],
                            stock_pos[1] + metrics::CARD_HEIGHT + 30.0,
                            30,
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
                            let stack = pred_hand_state.players()[idx].stack_at(held.src);
                            if let Some(stack) = stack {
                                let cards = stack.cards();
                                if (held.count as usize) <= cards.len() {
                                    let cards = &cards[(cards.len() - held.count as usize)..];

                                    if cards[0].card == held.top_card {
                                        ctx.draw_vertical_stack_cards(
                                            cards,
                                            screen_center.0 + mouse_pos[0] - held.offset.0,
                                            screen_center.1 + mouse_pos[1] - held.offset.1,
                                        );
                                    }
                                }
                            }
                        }

                        mq::draw_texture_ex(
                            &ctx.cursors_texture,
                            screen_center.0 + mouse_pos[0] - 1.0,
                            screen_center.1 + mouse_pos[1] - 1.0,
                            crate::PLAYER_COLORS
                                [(pred_hand_state.players()[idx].player_id() >> 4) as usize],
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
                }

                mq::set_camera(&normal_camera);

                if let Some(my_player_idx) = self.my_player_idx {
                    let my_player_state = &pred_hand_state.players()[my_player_idx];
                    if let Some(ref held) = hand_extra.my_held_state {
                        let stack = my_player_state.stack_at(held.info.src);
                        if let Some(stack) = stack {
                            let stack_cards = stack.cards();
                            if stack_cards.len() >= held.info.count as usize {
                                let cards =
                                    &stack_cards[(stack_cards.len() - held.info.count as usize)..];

                                ctx.draw_vertical_stack_cards(
                                    cards,
                                    mouse_pos[0] - held.info.offset.0,
                                    mouse_pos[1] - held.info.offset.1,
                                );
                            } else {
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
                        NERTS_OVERLAY_COLOR,
                    );

                    ctx.draw_text_centered(
                        "Nerts!",
                        screen_center.0,
                        screen_center.1,
                        100,
                        NERTS_TEXT_COLOR,
                    );
                }

                if !started {
                    mq::draw_rectangle(
                        0.0,
                        screen_center.1 - 70.0,
                        screen_size.0,
                        140.0,
                        NERTS_OVERLAY_COLOR,
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
                                NERTS_TEXT_COLOR,
                            );
                        }
                    }
                }

                egui_macroquad::ui(|egui_ctx| {
                    let ui_scale = scale / egui_ctx.zoom_factor();

                    egui::CentralPanel::default()
                        .frame(
                            egui::Frame::none()
                                .inner_margin(egui::style::Margin::same(super::SCREEN_MARGIN)),
                        )
                        .show(egui_ctx, |ui| {
                            ui.set_enabled(interaction_enabled);

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
                            });

                            ui.label(format!(
                                "Room Code: {}",
                                crate::util::to_full_game_id_str(shared.server_id, shared.game.id)
                            ));

                            if let Some(my_player_idx) = self.my_player_idx {
                                let my_player_state = &pred_hand_state.players()[my_player_idx];

                                if my_player_state.nerts_stack().len() < 1 {
                                    let location = metrics.player_loc(my_player_idx);
                                    let position = mq::Vec2::from(location.pos()) + mq::Vec2::from(screen_center);

                                    ui.allocate_ui_at_rect(
                                        egui::Rect {
                                            min: egui::Pos2::new(position[0], position[1]),
                                            max: egui::Pos2::new(
                                                position[0] + 12.0 * metrics::HORIZONTAL_STACK_SPACING + metrics::CARD_WIDTH,
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

                                                    if settings.nerts_callout {
                                                        macroquad::audio::play_sound_once(&ctx.nerts_callout);
                                                    }
                                                }
                                            });
                                        },
                                    );
                                }
                            }
                        });

                    if self.show_settings {
                        if !super::render_settings_window(&egui_ctx, &ctx.settings_mutex) {
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
                }
                .into()
            }
        } else {
            super::View::from_connection_state(&*lock)
        }
    }
}
