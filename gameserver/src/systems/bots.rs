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
                    use nertsio_ui_metrics::CARD_WIDTH;

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
                                game.settings.nerts_stack_size.into(),
                            );

                            for idx in 0..hand.players().len() {
                                let hand_player = &hand.players()[idx];
                                if let Some(player) = game.players.get_mut(&hand_player.player_id())
                                {
                                    let player_loc = metrics.player_loc(idx);

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
                                                            {
                                                                let possible_lake_targets = hand
                                                                    .lake_stacks()
                                                                    .iter()
                                                                    .enumerate()
                                                                    .filter_map(|(i, stack)| {
                                                                        if stack.can_add(*card) {
                                                                            Some(ni_ty::StackLocation::Lake(i as u16))
                                                                        } else {
                                                                            None
                                                                        }
                                                                    });

                                                                // Move to the closest lake stack

                                                                let from =
                                                                    ni_ty::StackLocation::Player(
                                                                        idx as u8, src,
                                                                    );
                                                                let from_pos =
                                                                    metrics.stack_pos(from);

                                                                if let Some(to) = possible_lake_targets.min_by_key(|target| {
                                                                    let to_pos = metrics.stack_pos(*target);
                                                                    let to_pos = if player_loc.inverted {
                                                                        (-to_pos.0 - CARD_WIDTH, to_pos.1)
                                                                    } else {
                                                                        to_pos
                                                                    };

                                                                    float_ord::FloatOrd((to_pos.0 - from_pos.0).powf(2.0) + (to_pos.1 - from_pos.1).powf(2.0))
                                                                }) {
                                                                    new_plan = Some(ni_ty::HandAction::Move { from, to, count: 1}.into());
                                                                }
                                                            }

                                                            if new_plan.is_none() {
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
                                                                            if dest_stack.can_add(back) && !dest_stack.is_empty() {
                                                                                new_plan = Some(ni_ty::HandAction::Move { from: ni_ty::StackLocation::Player(idx as u8, src), to: dest, count: count as u8 }.into());
                                                                                break;
                                                                            }
                                                                        }

                                                                        if new_plan.is_none() && !src_is_tableau {
                                                                            for (i, dest_stack) in hand_player
                                                                                .tableau_stacks()
                                                                                .iter()
                                                                                .enumerate()
                                                                            {
                                                                                let dest = ni_ty::StackLocation::Player(idx as u8, ni_ty::PlayerStackLocation::Tableau(i as u8));
                                                                                if dest_stack.can_add(back) {
                                                                                    new_plan = Some(ni_ty::HandAction::Move { from: ni_ty::StackLocation::Player(idx as u8, src), to: dest, count: count as u8 }.into());
                                                                                    break;
                                                                                }
                                                                            }
                                                                        }
                                                                    }
                                                                    _ => {}
                                                                }
                                                            }
                                                        }
                                                    }
                                                }

                                                if new_plan.is_none() {
                                                    let src_stack = hand_player.waste_stack();

                                                    if let Some(src_card) = src_stack.last() {
                                                        if let Some(nerts_stack_top) =
                                                            hand_player.nerts_stack().last()
                                                        {
                                                            for (i, dest_stack) in hand_player
                                                                .tableau_stacks()
                                                                .iter()
                                                                .enumerate()
                                                            {
                                                                if dest_stack.can_add(*src_card)
                                                                    && dest_stack.ordering().allows(
                                                                        src_card.card,
                                                                        nerts_stack_top.card,
                                                                    )
                                                                {
                                                                    new_plan = Some(ni_ty::HandAction::Move {
                                                                        from: ni_ty::StackLocation::Player(idx as u8, ni_ty::PlayerStackLocation::Waste),
                                                                        to: ni_ty::StackLocation::Player(idx as u8, ni_ty::PlayerStackLocation::Tableau(i as u8)),
                                                                        count: 1,
                                                                    }.into());
                                                                    break;
                                                                }
                                                            }
                                                        }

                                                        if new_plan.is_none() {
                                                            'outer: for (i, dest_stack) in
                                                                hand_player
                                                                    .tableau_stacks()
                                                                    .iter()
                                                                    .enumerate()
                                                            {
                                                                if dest_stack.can_add(*src_card) {
                                                                    for purpose_stack in
                                                                        hand_player.tableau_stacks()
                                                                    {
                                                                        if let Some(
                                                                            purpose_bottom,
                                                                        ) = purpose_stack.first()
                                                                        {
                                                                            if dest_stack
                                                                                .ordering()
                                                                                .allows(
                                                                                    src_card.card,
                                                                                    purpose_bottom
                                                                                        .card,
                                                                                )
                                                                            {
                                                                                new_plan = Some(ni_ty::HandAction::Move {
                                                                                    from: ni_ty::StackLocation::Player(idx as u8, ni_ty::PlayerStackLocation::Waste),
                                                                                    to: ni_ty::StackLocation::Player(idx as u8, ni_ty::PlayerStackLocation::Tableau(i as u8)),
                                                                                    count: 1,
                                                                                }.into());
                                                                                break 'outer;
                                                                            }
                                                                        }
                                                                    }
                                                                }
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
                                                                    held.src != from
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
                                                let get_dest_for_stack = |loc, take_count| {
                                                    metrics.get_dest_for_stack(
                                                        &hand,
                                                        loc,
                                                        take_count,
                                                        player_loc.inverted,
                                                    )
                                                };

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
                                                        if dest.contains(mouse_state.position) {
                                                            *plan = None;

                                                            Some(current_plan)
                                                        } else {
                                                            *target = dest.center();

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
                                                                if dest
                                                                    .contains(mouse_state.position)
                                                                {
                                                                    *plan = None;

                                                                    Some(current_plan)
                                                                } else {
                                                                    if hand
                                                                        .clone()
                                                                        .apply(
                                                                            Some(idx as u8),
                                                                            action,
                                                                        )
                                                                        .is_err()
                                                                    {
                                                                        *plan = None;
                                                                        None
                                                                    } else {
                                                                        *target = dest.center();
                                                                        None
                                                                    }
                                                                }
                                                            } else {
                                                                let dest = get_dest_for_stack(
                                                                    from,
                                                                    count.into(),
                                                                );
                                                                if dest
                                                                    .contains(mouse_state.position)
                                                                {
                                                                    let from_stack = hand
                                                                        .stack_at(from)
                                                                        .unwrap();
                                                                    if from_stack.len()
                                                                        >= count.into()
                                                                    {
                                                                        mouse_state.held = Some(ni_ty::HeldInfo {
                                                                        src: from,
                                                                        count,
                                                                        offset: (
                                                                            mouse_state.position.0 - dest.x,
                                                                            mouse_state.position.1 - dest.y
                                                                        ),
                                                                        top_card: from_stack.cards()[from_stack.len() - usize::from(count)].card,
                                                                    });

                                                                        *target =
                                                                            get_dest_for_stack(
                                                                                to, 0,
                                                                            )
                                                                            .center();
                                                                    } else {
                                                                        *plan = None;
                                                                    }
                                                                } else {
                                                                    *target = dest.center();
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

                                                            if dest.contains(mouse_state.position) {
                                                                *plan = None;

                                                                Some(current_plan)
                                                            } else {
                                                                *target = dest.center();

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
