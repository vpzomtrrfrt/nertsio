use crate::{GlobalState, PlayerController};
use nertsio_types as ni_ty;
use std::sync::Arc;

const STALL_SEND_COUNT: u8 = 6;

pub(crate) async fn run(global_state: Arc<GlobalState>) {
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(10));

    loop {
        interval.tick().await;

        global_state.games.retain(|_key, value| {
            !value
                .players
                .values()
                .all(|player| match player.controller {
                    PlayerController::Network { .. } => false,
                    PlayerController::Bot { .. } => true,
                })
        });

        global_state.games.alter_all(|_key, mut value| {
            if let Some(hand) = value.hand.as_mut() {
                hand.stalled_count += 1;
                if hand.sent_stall {
                    use rand::Rng;
                    let seed: u64 = rand::thread_rng().gen();
                    let action = ni_ty::HandAction::ShuffleStock { seed };
                    hand.hand.apply(None, action).unwrap();
                    hand.sent_stall = false;
                    hand.stalled_count = 0;

                    value.send_to_all(ni_ty::protocol::GameMessageS2C::ServerHandAction { action });
                } else {
                    if hand.stalled_count >= STALL_SEND_COUNT {
                        hand.sent_stall = true;
                        value.send_to_all(ni_ty::protocol::GameMessageS2C::HandStalled);
                    }
                }
            }
            value
        });
    }
}
