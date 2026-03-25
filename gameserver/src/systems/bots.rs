use crate::{BotPlan, GlobalState, PlayerController};
use nertsio_types as ni_ty;
use std::sync::Arc;

pub(crate) async fn run(global_state: Arc<GlobalState>) {
    futures_util::join!(
        {
            let global_state = global_state.clone();

            async move {
                let mut interval = tokio::time::interval(std::time::Duration::from_millis(100));

                loop {
                    use nertsio_ui_metrics::{
                        CARD_HEIGHT, CARD_WIDTH, HORIZONTAL_STACK_SPACING, NERTS_STACK_SPACING,
                        VERTICAL_STACK_SPACING,
                    };

                    interval.tick().await;

                    for mut game in global_state.games.iter_mut() {
                        if let Some(hand) = &game.hand {
                            if !hand.hand.started {
                                continue;
                            }

                            let hand = hand.hand.clone();

                            let metrics = nertsio_ui_metrics::HandMetrics::new(
                                hand.players().len(),
                                hand.players()[0].tableau_stacks().len(),
                                hand.lake_stacks().len(),
                                13,
                            );

                            for idx in 0..hand.players().len() {
                                let hand_player = &hand.players()[idx];
                                if let Some(player) = game.players.get_mut(&hand_player.player_id())
                                {
                                    let player_loc = metrics.player_loc(idx);

                                    let get_dest_for_stack = |loc, take_count| {
                                        let stack = hand.stack_at(loc).unwrap();
                                        let remaining_count = stack.len() - take_count;

                                        let stack_pos = metrics.stack_pos(loc);

                                        let stack_pos = match loc {
                                            ni_ty::StackLocation::Lake(_) => stack_pos,
                                            ni_ty::StackLocation::Player(_, loc) => match loc {
                                                ni_ty::PlayerStackLocation::Nerts => (
                                                    stack_pos.0
                                                        + (remaining_count as f32)
                                                            * NERTS_STACK_SPACING,
                                                    stack_pos.1,
                                                ),
                                                ni_ty::PlayerStackLocation::Tableau(_) => (
                                                    stack_pos.0,
                                                    stack_pos.1
                                                        + (remaining_count as f32)
                                                            * VERTICAL_STACK_SPACING,
                                                ),
                                                ni_ty::PlayerStackLocation::Stock => stack_pos,
                                                ni_ty::PlayerStackLocation::Waste => {
                                                    let remaining_visible =
                                                        stack.len().min(3) - take_count;
                                                    (
                                                        stack_pos.0
                                                            + (remaining_visible as f32)
                                                                * HORIZONTAL_STACK_SPACING,
                                                        stack_pos.1,
                                                    )
                                                }
                                            },
                                        };

                                        let stack_pos = if let ni_ty::StackLocation::Lake(_) = loc {
                                            if player_loc.inverted {
                                                (-stack_pos.0 - CARD_WIDTH, stack_pos.1)
                                            } else {
                                                stack_pos
                                            }
                                        } else {
                                            stack_pos
                                        };

                                        (
                                            stack_pos.0 + CARD_WIDTH / 2.0,
                                            stack_pos.1 + CARD_HEIGHT / 2.0,
                                        )
                                    };

                                    let reached = |a: (f32, f32), b: (f32, f32)| {
                                        a.0 > b.0 - CARD_WIDTH / 2.0
                                            && a.0 < b.0 + CARD_WIDTH / 2.0
                                            && a.1 > b.1 - CARD_HEIGHT / 2.0
                                            && a.1 < b.1 + CARD_HEIGHT / 2.0
                                    };

                                    if let PlayerController::Bot {
                                        ref mut plan,
                                        ref mut mouse_state,
                                        ref mut target,
                                        ..
                                    } = &mut player.controller
                                    {
                                        let action = match plan {
                                            None => {
                                                // make a new plan

                                                let mut new_plan = None;

                                                if hand_player.nerts_stack().is_empty() {
                                                    new_plan = Some(BotPlan::CallNerts);
                                                }

                                                if new_plan.is_none() {
                                                    for src in std::iter::once(
                                                        ni_ty::PlayerStackLocation::Nerts,
                                                    )
                                                    .chain(
                                                        (0..hand_player.tableau_stacks().len())
                                                            .map(|i| {
                                                                ni_ty::PlayerStackLocation::Tableau(
                                                                    i as u8,
                                                                )
                                                            }),
                                                    )
                                                    .chain(std::iter::once(
                                                        ni_ty::PlayerStackLocation::Waste,
                                                    )) {
                                                        let stack =
                                                            hand_player.stack_at(src).unwrap();
                                                        if let Some(card) = stack.last() {
                                                            for (i, stack) in hand
                                                                .lake_stacks()
                                                                .iter()
                                                                .enumerate()
                                                            {
                                                                if stack.can_add(*card) {
                                                                    new_plan = Some(ni_ty::HandAction::Move { from: ni_ty::StackLocation::Player(idx as u8, src), to: ni_ty::StackLocation::Lake(i as u16), count: 1}.into());
                                                                    break;
                                                                }
                                                            }

                                                            match src {
                                                                ni_ty::PlayerStackLocation::Tableau(
                                                                    _,
                                                                )
                                                                | ni_ty::PlayerStackLocation::Nerts => {
                                                                    let src_is_tableau = matches!(src, ni_ty::PlayerStackLocation::Tableau(_));
                                                                    let count = if src_is_tableau {
                                                                        stack.len()
                                                                    } else {
                                                                        1
                                                                    };
                                                                    let back = stack.cards()
                                                                        [stack.len() - count];

                                                                    for (i, dest_stack) in hand_player
                                                                        .tableau_stacks()
                                                                        .iter()
                                                                        .enumerate()
                                                                    {
                                                                        let dest = ni_ty::StackLocation::Player(idx as u8, ni_ty::PlayerStackLocation::Tableau(i as u8));
                                                                        if dest_stack.can_add(back)
                                                                            && (!src_is_tableau
                                                                                || !dest_stack.is_empty())
                                                                        {
                                                                            new_plan = Some(ni_ty::HandAction::Move { from: ni_ty::StackLocation::Player(idx as u8, src), to: dest, count: count as u8 }.into());
                                                                            break;
                                                                        }
                                                                    }
                                                                }
                                                                _ => {}
                                                            }
                                                        }
                                                    }
                                                }

                                                if new_plan.is_none() {
                                                    if !hand_player.stock_stack().is_empty() {
                                                        new_plan = Some(
                                                            ni_ty::HandAction::FlipStock.into(),
                                                        );
                                                    } else if !hand_player.waste_stack().is_empty()
                                                    {
                                                        new_plan = Some(
                                                            ni_ty::HandAction::ReturnStock.into(),
                                                        );
                                                    }
                                                }

                                                if let Some(new_plan) = new_plan {
                                                    if let Some(held) = mouse_state.held {
                                                        if match new_plan {
                                                            BotPlan::CallNerts => true,
                                                            BotPlan::Action(action) => match action {
                                                                ni_ty::HandAction::ShuffleStock {
                                                                    ..
                                                                } => unreachable!(),
                                                                ni_ty::HandAction::FlipStock
                                                                | ni_ty::HandAction::ReturnStock => {
                                                                    true
                                                                }
                                                                ni_ty::HandAction::Move {
                                                                    from,
                                                                    count,
                                                                    ..
                                                                } => {
                                                                    ni_ty::StackLocation::Player(
                                                                        idx as u8, held.src,
                                                                    ) != from
                                                                        || held.count != count
                                                                }
                                                            }
                                                        } {
                                                            mouse_state.held = None;
                                                        }
                                                    }

                                                    *plan = Some(new_plan);
                                                }

                                                None
                                            }
                                            Some(current_plan) => {
                                                let current_plan = current_plan.clone();
                                                match current_plan {
                                                    BotPlan::CallNerts => {
                                                        let dest = get_dest_for_stack(
                                                            ni_ty::StackLocation::Player(
                                                                idx as u8,
                                                                ni_ty::PlayerStackLocation::Nerts,
                                                            ),
                                                            0,
                                                        );
                                                        if reached(mouse_state.position, dest) {
                                                            *plan = None;

                                                            Some(current_plan)
                                                        } else {
                                                            *target = dest;

                                                            None
                                                        }
                                                    }
                                                    BotPlan::Action(action) => match action {
                                                        ni_ty::HandAction::ShuffleStock {
                                                            ..
                                                        } => {
                                                            unreachable!()
                                                        }
                                                        ni_ty::HandAction::Move {
                                                            from,
                                                            to,
                                                            count,
                                                        } => {
                                                            if mouse_state.held.is_some() {
                                                                let dest =
                                                                    get_dest_for_stack(to, 0);
                                                                if reached(
                                                                    mouse_state.position,
                                                                    dest,
                                                                ) {
                                                                    *plan = None;

                                                                    Some(current_plan)
                                                                } else {
                                                                    *target = dest;
                                                                    None
                                                                }
                                                            } else {
                                                                let dest = get_dest_for_stack(
                                                                    from,
                                                                    count.into(),
                                                                );
                                                                if reached(
                                                                    mouse_state.position,
                                                                    dest,
                                                                ) {
                                                                    let from_stack = hand
                                                                        .stack_at(from)
                                                                        .unwrap();
                                                                    if from_stack.len()
                                                                        >= count.into()
                                                                    {
                                                                        mouse_state.held = Some(ni_ty::HeldInfo {
                                                                        src: if let ni_ty::StackLocation::Player(_, loc) = from {
                                                                            loc
                                                                        } else {
                                                                            panic!("somehow picked up a non-player stack")
                                                                        },
                                                                        count,
                                                                        offset: (
                                                                            mouse_state.position.0 - (dest.0 - CARD_WIDTH / 2.0),
                                                                            mouse_state.position.1 - (dest.1 - CARD_HEIGHT / 2.0),
                                                                        ),
                                                                        top_card: from_stack.cards()[from_stack.len() - usize::from(count)].card,
                                                                    });

                                                                        *target =
                                                                            get_dest_for_stack(
                                                                                to, 0,
                                                                            );
                                                                    } else {
                                                                        *plan = None;
                                                                    }
                                                                } else {
                                                                    *target = dest;
                                                                }

                                                                None
                                                            }
                                                        }
                                                        ni_ty::HandAction::FlipStock
                                                        | ni_ty::HandAction::ReturnStock => {
                                                            let dest = get_dest_for_stack(
                                                                ni_ty::StackLocation::Player(
                                                                    idx as u8,
                                                                    ni_ty::PlayerStackLocation::Stock,
                                                                ),
                                                                0,
                                                            );

                                                            if reached(mouse_state.position, dest) {
                                                                *plan = None;

                                                                Some(current_plan)
                                                            } else {
                                                                *target = dest;

                                                                None
                                                            }
                                                        }
                                                    },
                                                }
                                            }
                                        };

                                        if let Some(action) = action {
                                            match action {
                                                BotPlan::CallNerts => {
                                                    game.handle_nerts_call(
                                                        hand_player.player_id(),
                                                        &global_state,
                                                    );
                                                }
                                                BotPlan::Action(action) => {
                                                    if game
                                                        .hand
                                                        .as_mut()
                                                        .unwrap()
                                                        .hand
                                                        .apply(Some(idx as u8), action)
                                                        .is_ok()
                                                    {
                                                        game.send_to_all(
                                                            ni_ty::protocol::GameMessageS2C::PlayerHandAction {
                                                                player: idx as u8,
                                                                action,
                                                            },
                                                        );
                                                        if action.should_reset_stall() {
                                                            let hand_state =
                                                                game.hand.as_mut().unwrap();
                                                            hand_state.stalled_count = 0;
                                                            if hand_state.sent_stall {
                                                                hand_state.sent_stall = false;
                                                                game.send_to_all(ni_ty::protocol::GameMessageS2C::HandStallCancel);
                                                            }
                                                        }

                                                        let hand_player = &hand.players()[idx];
                                                        if let Some(player) = game
                                                            .players
                                                            .get_mut(&hand_player.player_id())
                                                        {
                                                            if let PlayerController::Bot {
                                                                ref mut mouse_state,
                                                                ..
                                                            } = &mut player.controller
                                                            {
                                                                mouse_state.held = None;
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        },
        {
            async move {
                let mut interval = tokio::time::interval(std::time::Duration::from_millis(50));

                loop {
                    interval.tick().await;

                    for mut game in global_state.games.iter_mut() {
                        let speed = game.settings.bot_difficulty * 60.0 + 5.0;

                        if let Some(hand) = &game.hand {
                            let player_count = hand.hand.players().len();

                            for idx in 0..player_count {
                                let player_id =
                                    game.hand.as_ref().unwrap().hand.players()[idx].player_id();
                                if let Some(player) = game.players.get_mut(&player_id) {
                                    if let PlayerController::Bot {
                                        ref mut mouse_state,
                                        ref mut target,
                                        ref mut seq,
                                        ref plan,
                                    } = &mut player.controller
                                    {
                                        if plan.is_some() {
                                            let dist = ((mouse_state.position.0 - target.0)
                                                .powf(2.0)
                                                + (mouse_state.position.1 - target.1).powf(2.0))
                                            .sqrt();

                                            if dist > 0.0 {
                                                if dist > speed {
                                                    mouse_state.position = (
                                                        mouse_state.position.0
                                                            + (target.0 - mouse_state.position.0)
                                                                / dist
                                                                * speed,
                                                        mouse_state.position.1
                                                            + (target.1 - mouse_state.position.1)
                                                                / dist
                                                                * speed,
                                                    );
                                                } else {
                                                    mouse_state.position = *target;
                                                }

                                                *seq += 1;

                                                let out_msg: bytes::Bytes = bincode::serialize(&ni_ty::protocol::DatagramMessageS2C::UpdateMouseState {
                                                    player_idx: idx as u8,
                                                    seq: *seq,
                                                    state: mouse_state.clone(),
                                                }).unwrap().into();

                                                for (id, server_player_state) in &game.players {
                                                    if *id != player_id {
                                                        if let PlayerController::Network {
                                                            ref connection,
                                                            ..
                                                        } = server_player_state.controller
                                                        {
                                                            if let Err(err) = connection
                                                                .send_datagram(out_msg.clone())
                                                            {
                                                                eprintln!("Failed to queue update to player: {:?}", err);
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        },
    );
}
