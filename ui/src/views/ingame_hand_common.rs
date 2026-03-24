use crate::settings::DragMode;
use macroquad::logging as log;
use macroquad::prelude as mq;
use nertsio_types as ni_ty;
use nertsio_ui_metrics as metrics;

pub const NERTS_OVERLAY_COLOR: mq::Color = mq::Color::new(1.0, 1.0, 1.0, 0.4);
pub const NERTS_TEXT_COLOR: mq::Color = mq::Color::new(0.0, 0.0, 1.0, 1.0);

pub fn hand_metrics(hand: &ni_ty::HandState) -> metrics::HandMetrics {
    metrics::HandMetrics::new(
        hand.players().len(),
        hand.players()[0].tableau_stacks().len(),
        hand.lake_stacks().len(),
    )
}

pub fn screen_size_for_hand(
    real_screen_size: (f32, f32),
    metrics: &metrics::HandMetrics,
) -> (f32, f32) {
    let needed_screen_width = metrics.needed_screen_width();
    let needed_screen_height = metrics.needed_screen_height();

    let mut factor =
        (real_screen_size.0 / needed_screen_width).min(real_screen_size.1 / needed_screen_height);

    if factor > 1.0 {
        // round down to nearest 0.5
        factor = (factor * 2.0).floor() / 2.0;
    }

    (real_screen_size.0 / factor, real_screen_size.1 / factor)
}

pub fn draw_player_stacks(
    ctx: &super::GameContext,
    player_state: &ni_ty::HandPlayerState,
    held_info: &Option<ni_ty::HeldInfo>,
    metrics: &metrics::HandMetrics,
    location: metrics::PlayerLocation,
    screen_center: mq::Vec2,
    started: bool,
    start_animation_progress: f32,
) {
    let mut animation_not_consumed = start_animation_progress;

    let stock_pos =
        mq::Vec2::from(metrics.player_stack_pos(ni_ty::PlayerStackLocation::Stock, location))
            + screen_center;

    if player_state.stock_stack().is_empty() {
        ctx.draw_placeholder(stock_pos[0], stock_pos[1]);
    } else {
        ctx.draw_back(stock_pos[0], stock_pos[1], player_state.player_id());
    }

    if !player_state.nerts_stack().is_empty() {
        let nerts_stack_pos =
            mq::Vec2::from(metrics.player_stack_pos(ni_ty::PlayerStackLocation::Nerts, location))
                + screen_center;
        let card = player_state.nerts_stack().last().unwrap();

        if started {
            for i in 0..(player_state.nerts_stack().len() - 1) {
                ctx.draw_back(
                    nerts_stack_pos[0] + (i as f32) * metrics::NERTS_STACK_SPACING,
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
                            * metrics::NERTS_STACK_SPACING,
                    nerts_stack_pos[1],
                );
            }
        } else {
            for i in 0..player_state.nerts_stack().len() {
                let target_pos = mq::Vec2::new(
                    nerts_stack_pos[0] + (i as f32) * metrics::NERTS_STACK_SPACING,
                    nerts_stack_pos[1],
                );

                let dist = stock_pos.distance(target_pos);

                animation_not_consumed -= dist;

                if animation_not_consumed < 0.0 {
                    log::debug!("portion = {}", 1.0 - animation_not_consumed / dist);
                    let pos = stock_pos.lerp(target_pos, 1.0 + animation_not_consumed / dist);
                    ctx.draw_back(pos.x, pos.y, player_state.player_id());
                    break;
                } else {
                    ctx.draw_back(target_pos.x, target_pos.y, player_state.player_id());
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
            if i == (*stack_idx as usize) {
                if *count as usize <= cards.len() {
                    &cards[..(cards.len() - *count as usize)]
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
        let target_pos = mq::Vec2::from(metrics.player_stack_pos(loc, location)) + screen_center;

        if started {
            ctx.draw_vertical_stack_cards(cards, target_pos[0], target_pos[1]);
        } else {
            if animation_not_consumed > 0.0 {
                let dist = stock_pos.distance(target_pos);

                animation_not_consumed -= dist;

                if animation_not_consumed < 0.0 {
                    let pos = stock_pos.lerp(target_pos, 1.0 + animation_not_consumed / dist);

                    ctx.draw_back(pos[0], pos[1], player_state.player_id());
                } else {
                    ctx.draw_back(target_pos[0], target_pos[1], player_state.player_id());
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
        if *count as usize <= waste_cards.len() {
            &waste_cards[..(waste_cards.len() - *count as usize)]
        } else {
            waste_cards
        }
    } else {
        waste_cards
    };

    if !waste_cards.is_empty() {
        let waste_pos =
            mq::Vec2::from(metrics.player_stack_pos(ni_ty::PlayerStackLocation::Waste, location))
                + screen_center;

        ctx.draw_horizontal_stack_cards(waste_cards, waste_pos[0], waste_pos[1]);
    }
}

pub fn draw_held_state(
    ctx: &super::GameContext,
    hand: &ni_ty::HandState,
    player_idx: usize,
    held: ni_ty::HeldInfo,
    mouse_pos: mq::Vec2,
) {
    let stack = hand.players()[player_idx].stack_at(held.src);
    if let Some(stack) = stack {
        let cards = stack.cards();
        if (held.count as usize) <= cards.len() {
            let cards = &cards[(cards.len() - held.count as usize)..];

            if cards[0].card == held.top_card {
                ctx.draw_vertical_stack_cards(
                    cards,
                    mouse_pos[0] - held.offset.0,
                    mouse_pos[1] - held.offset.1,
                );
            }
        }
    }
}

pub fn handle_input(
    ctx: &super::GameContext,
    settings: &crate::Settings,
    metrics: &metrics::HandMetrics,
    screen_center: mq::Vec2,
    my_player_idx: usize,
    hand: &ni_ty::HandState,
    my_held_state: &mut Option<crate::HeldState>,
    mouse_pos: mq::Vec2,
) -> Option<ni_ty::HandAction> {
    let my_player_idx_u8 = my_player_idx as u8;
    let player_state = &hand.players()[my_player_idx];

    let mouse_pressed = mq::is_mouse_button_pressed(mq::MouseButton::Left);
    let my_location = metrics.player_loc(my_player_idx);

    if mouse_pressed
        || (mq::is_mouse_button_released(mq::MouseButton::Left)
            && match settings.drag_mode {
                DragMode::Click => false,
                DragMode::Drag | DragMode::Hybrid => true,
            })
    {
        let nerts_stack_pos = mq::Vec2::from(
            metrics.player_stack_pos(ni_ty::PlayerStackLocation::Nerts, my_location),
        ) + screen_center;
        let stock_stack_pos = mq::Vec2::from(
            metrics.player_stack_pos(ni_ty::PlayerStackLocation::Stock, my_location),
        ) + screen_center;
        let waste_stack_pos = mq::Vec2::from(
            metrics.player_stack_pos(ni_ty::PlayerStackLocation::Waste, my_location),
        ) + screen_center;

        if mq::Rect::new(
            stock_stack_pos[0],
            stock_stack_pos[1],
            metrics::CARD_WIDTH,
            metrics::CARD_HEIGHT,
        )
        .contains(mouse_pos)
        {
            if mouse_pressed {
                let action = if player_state.stock_stack().is_empty() {
                    ni_ty::HandAction::ReturnStock
                } else {
                    ni_ty::HandAction::FlipStock
                };

                if hand.clone().apply(Some(my_player_idx_u8), action).is_ok() {
                    *my_held_state = None;

                    return Some(action);
                }
            }
        } else {
            #[derive(Debug)]
            struct CandidateStack {
                location: ni_ty::StackLocation,
                distance: f32,
                pickup_details: Option<(usize, mq::Vec2)>,
            }

            // oof this is complicated
            struct FoundChecker {
                found: Vec<CandidateStack>,
                mouse_pos: mq::Vec2,
                held_rect: Option<mq::Rect>,
            }

            impl FoundChecker {
                fn check_stack(
                    &mut self,
                    rect: mq::Rect,
                    loc: ni_ty::StackLocation,
                    f: impl FnOnce() -> (usize, mq::Vec2),
                ) {
                    log::debug!("checking stack {:?} {:?}", rect, self.held_rect);

                    if rect.contains(self.mouse_pos) {
                        self.found.push(CandidateStack {
                            location: loc,
                            distance: 0.0,
                            pickup_details: Some(f()),
                        });
                    } else if let Some(held_rect) = self.held_rect {
                        if rect.overlaps(&held_rect) {
                            // from https://stackoverflow.com/a/18157551/2533397
                            let dx = (rect.x - self.mouse_pos.x)
                                .max(0.0)
                                .max(self.mouse_pos.x - (rect.x + rect.w));
                            let dy = (rect.y - self.mouse_pos.y)
                                .max(0.0)
                                .max(self.mouse_pos.y - (rect.y + rect.h));

                            self.found.push(CandidateStack {
                                location: loc,
                                distance: dx * dx + dy * dy,
                                pickup_details: None,
                            });
                        }
                    }
                }
            }

            let mut checker = FoundChecker {
                found: Vec::new(),
                mouse_pos,
                held_rect: my_held_state.as_ref().map(|held| {
                    mq::Rect::new(
                        mouse_pos[0] - held.info.offset.0,
                        mouse_pos[1] - held.info.offset.1,
                        metrics::CARD_WIDTH,
                        metrics::CARD_HEIGHT,
                    )
                }),
            };

            if !player_state.nerts_stack().is_empty() {
                checker.check_stack(
                    mq::Rect::new(
                        nerts_stack_pos[0]
                            + ((player_state.nerts_stack().len() - 1) as f32)
                                * metrics::NERTS_STACK_SPACING,
                        nerts_stack_pos[1],
                        metrics::CARD_WIDTH,
                        metrics::CARD_HEIGHT,
                    ),
                    ni_ty::StackLocation::Player(
                        my_player_idx_u8,
                        ni_ty::PlayerStackLocation::Nerts,
                    ),
                    || {
                        (
                            1,
                            mouse_pos
                                - mq::Vec2::new(
                                    nerts_stack_pos[0]
                                        + ((player_state.nerts_stack().len() - 1) as f32)
                                            * metrics::NERTS_STACK_SPACING,
                                    nerts_stack_pos[1],
                                ),
                        )
                    },
                );
            }

            for stack_idx in 0..hand.lake_stacks().len() {
                let loc = ni_ty::StackLocation::Lake(stack_idx as u16);

                let stack_pos = mq::Vec2::from(metrics.stack_pos(loc));
                let stack_pos = if my_location.inverted {
                    mq::Vec2::new(
                        -stack_pos[0] - (metrics::CARD_WIDTH + metrics::LAKE_SPACING),
                        stack_pos[1],
                    )
                } else {
                    stack_pos
                } + screen_center;

                checker.check_stack(
                    mq::Rect::new(
                        stack_pos[0],
                        stack_pos[1],
                        metrics::CARD_WIDTH,
                        metrics::CARD_HEIGHT,
                    ),
                    loc,
                    || (1, mouse_pos - stack_pos),
                );
            }

            if !player_state.waste_stack().is_empty() {
                let top_pos = mq::Vec2::new(
                    waste_stack_pos[0]
                        + (metrics::HORIZONTAL_STACK_SPACING
                            * ((player_state.waste_stack().len().min(3) - 1) as f32)),
                    waste_stack_pos[1],
                );

                checker.check_stack(
                    mq::Rect::new(
                        top_pos[0],
                        top_pos[1],
                        metrics::CARD_WIDTH,
                        metrics::CARD_HEIGHT,
                    ),
                    ni_ty::StackLocation::Player(
                        my_player_idx_u8,
                        ni_ty::PlayerStackLocation::Waste,
                    ),
                    || (1, mouse_pos - top_pos),
                );
            }

            for (i, stack) in player_state.tableau_stacks().iter().enumerate() {
                let loc = ni_ty::PlayerStackLocation::Tableau(i as u8);

                let stack_pos =
                    mq::Vec2::from(metrics.player_stack_pos(loc, my_location)) + screen_center;

                checker.check_stack(
                    mq::Rect::new(
                        stack_pos[0],
                        stack_pos[1],
                        metrics::CARD_WIDTH,
                        metrics::CARD_HEIGHT
                            + ((stack.len() as f32) - 1.0) * metrics::VERTICAL_STACK_SPACING,
                    ),
                    ni_ty::StackLocation::Player(
                        my_player_idx_u8,
                        ni_ty::PlayerStackLocation::Tableau(i as u8),
                    ),
                    || {
                        if stack.is_empty() {
                            (0, mouse_pos - stack_pos)
                        } else {
                            let found_idx = (((mouse_pos[1] - stack_pos[1])
                                / metrics::VERTICAL_STACK_SPACING)
                                as usize)
                                .min(stack.len() - 1);

                            (
                                stack.len() - found_idx,
                                mouse_pos
                                    - mq::Vec2::new(
                                        stack_pos[0],
                                        stack_pos[1]
                                            + ((found_idx as f32)
                                                * metrics::VERTICAL_STACK_SPACING),
                                    ),
                            )
                        }
                    },
                );
            }

            let _ = player_state;

            let found = checker.found;

            log::debug!("clicks found {:?}", found);

            match my_held_state {
                None => {
                    if mouse_pressed {
                        // if not held, there should be at most one found since
                        // we only check mouse_pos in that case
                        //
                        // count & offset should also be guaranteed

                        if let Some(found) = found.first() {
                            if let ni_ty::StackLocation::Player(_, src) = found.location {
                                let stack = hand.stack_at(found.location).unwrap();
                                if !stack.is_empty() {
                                    let (count, offset) = found.pickup_details.unwrap();

                                    let top_card = stack.cards()[stack.cards().len() - count].card;

                                    *my_held_state = Some(crate::HeldState {
                                        info: ni_ty::HeldInfo {
                                            src,
                                            count: count as u8,
                                            offset: (offset[0], offset[1]),
                                            top_card,
                                        },
                                        is_drag: match settings.drag_mode {
                                            DragMode::Click => false,
                                            DragMode::Drag | DragMode::Hybrid => true,
                                        },
                                    });

                                    if settings.sounds {
                                        macroquad::audio::play_sound_once(ctx.pickup_sound);
                                    }
                                }
                            }
                        }
                    }
                }
                Some(ref mut held) => {
                    let src_loc = ni_ty::StackLocation::Player(my_player_idx_u8, held.info.src);

                    let found = {
                        let mut found = found;
                        found.sort_by(|a, b| a.distance.partial_cmp(&b.distance).unwrap());
                        found
                    };

                    let mut chosen_action: Option<ni_ty::HandAction> = None;
                    let mut any_success = false;
                    let mut maybe_start_nondrag = false;
                    let mut put_back = false;

                    for found in found {
                        let target_loc = found.location;
                        if target_loc == src_loc {
                            any_success = true;
                            maybe_start_nondrag = true;
                            put_back = true;
                            break;
                        } else {
                            chosen_action = if matches!(
                                target_loc,
                                ni_ty::StackLocation::Player(
                                    _,
                                    ni_ty::PlayerStackLocation::Tableau(_)
                                ) | ni_ty::StackLocation::Lake(_)
                            ) {
                                if let Some(target_stack) = hand.stack_at(target_loc) {
                                    if let Some(src_stack) = hand.stack_at(src_loc) {
                                        let stack_cards = src_stack.cards();
                                        let back_card = &stack_cards
                                            [stack_cards.len() - held.info.count as usize];

                                        if target_stack.can_add(*back_card) {
                                            let action = ni_ty::HandAction::Move {
                                                from: src_loc,
                                                count: held.info.count,
                                                to: target_loc,
                                            };

                                            log::debug!("applying for check");
                                            if hand
                                                .clone()
                                                .apply(Some(my_player_idx_u8), action)
                                                .is_ok()
                                            {
                                                // should always be
                                                // true?

                                                Some(action)
                                            } else {
                                                None
                                            }
                                        } else {
                                            log::debug!(
                                                "can't add {:?} to {:?}",
                                                back_card,
                                                target_stack
                                            );

                                            None
                                        }
                                    } else {
                                        None
                                    }
                                } else {
                                    None
                                }
                            } else {
                                None
                            };

                            if chosen_action.is_some() {
                                any_success = true;
                                break;
                            }
                        }
                    }

                    if maybe_start_nondrag
                        && !mouse_pressed
                        && held.is_drag
                        && settings.drag_mode == DragMode::Hybrid
                    {
                        held.is_drag = false;
                    } else if any_success || held.is_drag {
                        let is_drag = held.is_drag;

                        *my_held_state = None;

                        if let Some(action) = chosen_action {
                            return Some(action);
                        } else if ((!any_success && is_drag) || put_back) && settings.sounds {
                            // not a real move, make sure to play the sound
                            // anyway
                            macroquad::audio::play_sound_once(ctx.place_sound);
                        }
                    }
                }
            }
        }
    } else if mq::is_mouse_button_pressed(mq::MouseButton::Right)
        || mq::is_key_pressed(mq::KeyCode::Escape)
    {
        if my_held_state.is_some() {
            *my_held_state = None;

            if settings.sounds {
                macroquad::audio::play_sound_once(ctx.place_sound);
            }
        }
    } else if mq::is_key_pressed(mq::KeyCode::Tab)
        || mq::is_key_pressed(mq::KeyCode::Z)
        || mq::is_key_pressed(mq::KeyCode::X)
    {
        let action = if player_state.stock_stack().is_empty() {
            ni_ty::HandAction::ReturnStock
        } else {
            ni_ty::HandAction::FlipStock
        };

        if hand.clone().apply(Some(my_player_idx_u8), action).is_ok() {
            *my_held_state = None;

            return Some(action);
        }
    }

    None
}
