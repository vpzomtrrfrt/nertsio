use crate::{GlobalState, MAX_PLAYERS};
use nertsio_types as ni_ty;
use std::borrow::Cow;
use std::sync::Arc;

pub(crate) async fn run(
    global_state: Arc<GlobalState>,
    my_address_ipv4: std::net::SocketAddrV4,
    my_hostname: Option<String>,
    web_port: u16,
    server_id: u8,
    redis_conn: redis_async::client::PairedConnection,
) {
    futures_util::join!(
        {
            let global_state = global_state.clone();
            let redis_conn = &redis_conn;
            async move {
                let mut interval = tokio::time::interval(std::time::Duration::from_secs(2));

                loop {
                    interval.tick().await;

                    let status = ni_ty::protocol::ServerStatusMessage {
                        server_id,
                        address_ipv4: my_address_ipv4,
                        hostname: my_hostname.as_deref().map(Cow::Borrowed),
                        min_protocol_version: crate::MIN_PROTOCOL_VERSION,
                        protocol_version: ni_ty::protocol::PROTOCOL_VERSION,
                        web_port: Some(web_port),
                        open_public_games: global_state
                            .games
                            .iter()
                            .filter(|entry| {
                                entry.value().public && entry.value().players.len() < MAX_PLAYERS
                            })
                            .map(|entry| ni_ty::protocol::PublicGameInfo {
                                game_id: *entry.key(),
                                players: entry.value().players.len() as u8,
                                real_players: Some(
                                    entry
                                        .value()
                                        .players
                                        .values()
                                        .filter(|x| {
                                            matches!(
                                                x.controller,
                                                crate::PlayerController::Network { .. }
                                            )
                                        })
                                        .count() as u8,
                                ),
                                waiting: entry.value().hand.is_none(),
                                active_players: Some(
                                    entry
                                        .value()
                                        .players
                                        .values()
                                        .filter(|x| !x.spectating)
                                        .count() as u8,
                                ),
                                max_players: Some(entry.value().settings.max_players),
                            })
                            .collect(),
                        stats: global_state.games.iter().fold(
                            ni_ty::protocol::ServerStats {
                                public_games: 0,
                                private_games: 0,
                                public_game_players: 0,
                                private_game_players: 0,
                            },
                            |mut acc, entry| {
                                if entry.value().public {
                                    acc.public_games += 1;
                                    acc.public_game_players += entry.value().players.len() as u32;
                                } else {
                                    acc.private_games += 1;
                                    acc.private_game_players += entry.value().players.len() as u32;
                                }

                                acc
                            },
                        ),
                    };

                    if let Err(err) = redis_conn
                        .send::<i64>(redis_async::resp_array!(
                            "PUBLISH",
                            ni_ty::protocol::COORDINATOR_CHANNEL,
                            serde_json::to_vec(&status).unwrap()
                        ))
                        .await
                    {
                        eprintln!("failed to publish status: {:?}", err);
                    }
                }
            }
        },
        {
            let redis_conn = &redis_conn;
            async move {
                let mut interval = tokio::time::interval(std::time::Duration::from_secs(100));

                loop {
                    interval.tick().await;

                    if let Err(err) = redis_conn
                        .send::<String>(redis_async::resp_array!(
                            "SET",
                            format!("server_ids/{}", server_id),
                            "yes",
                            "EX",
                            "120",
                        ))
                        .await
                    {
                        eprintln!("failed to renew ID reservation: {:?}", err);
                    }
                }
            }
        }
    );
}
