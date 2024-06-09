use futures_util::{TryFutureExt, TryStreamExt};
use nertsio_types as ni_ty;
use serde::Deserialize;
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

const GAMESERVER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(4);
const PUBLIC_GAME_COUNT: usize = 8;

#[derive(Debug)]
pub enum Error {
    Internal(Box<dyn std::error::Error + Send>),
    InternalStr(String),
    InternalStrStatic(&'static str),
    UserError(hyper::Response<hyper::Body>),
    RoutingError(trout::RoutingFailure),
}

impl<T: 'static + std::error::Error + Send> From<T> for Error {
    fn from(err: T) -> Error {
        Error::Internal(Box::new(err))
    }
}

struct GlobalState {
    gameservers: RwLock<
        HashMap<
            u8,
            (
                std::time::Instant,
                ni_ty::protocol::ServerStatusMessage<'static>,
            ),
        >,
    >,
}

type RouteNode<P> = trout::Node<
    P,
    hyper::Request<hyper::Body>,
    std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<hyper::Response<hyper::Body>, Error>> + Send>,
    >,
    Arc<GlobalState>,
>;

pub fn common_response_builder() -> http::response::Builder {
    hyper::Response::builder().header(hyper::header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
}

pub fn simple_response(
    code: hyper::StatusCode,
    text: impl Into<hyper::Body>,
) -> hyper::Response<hyper::Body> {
    common_response_builder()
        .status(code)
        .body(text.into())
        .unwrap()
}

pub fn json_response(body: &impl serde::Serialize) -> Result<hyper::Response<hyper::Body>, Error> {
    let body = serde_json::to_vec(&body)?;
    Ok(common_response_builder()
        .header(hyper::header::CONTENT_TYPE, "application/json")
        .body(body.into())?)
}

async fn handler_public_games_list(
    _: (),
    ctx: Arc<GlobalState>,
    _req: hyper::Request<hyper::Body>,
) -> Result<hyper::Response<hyper::Body>, crate::Error> {
    use rand::seq::IteratorRandom;

    let gameservers = ctx.gameservers.read().unwrap();

    let list_games = gameservers
        .iter()
        .flat_map(|(server_id, (_, info))| {
            let server_address_ipv4 = info.address_ipv4;
            let server_hostname = info.hostname.as_deref();
            let server_web_port = info.web_port;
            info.open_public_games
                .iter()
                .map(move |game| ni_ty::protocol::PublicGameInfoExpanded {
                    game_id: game.game_id,
                    players: game.players,
                    waiting: game.waiting,
                    server: ni_ty::protocol::ServerConnectionInfo {
                        server_id: *server_id,
                        address_ipv4: server_address_ipv4,
                        hostname: server_hostname.map(Cow::Borrowed),
                        web_port: server_web_port,
                    },
                })
        })
        .choose_multiple(&mut rand::thread_rng(), PUBLIC_GAME_COUNT);

    let info = ni_ty::protocol::RespList { items: list_games };

    json_response(&info)
}

fn default_protocol_version() -> u16 {
    3 // version before we started sending this
}

async fn handler_servers_pick_for_new_game(
    _: (),
    ctx: Arc<GlobalState>,
    req: hyper::Request<hyper::Body>,
) -> Result<hyper::Response<hyper::Body>, crate::Error> {
    use rand::seq::IteratorRandom;

    #[derive(Deserialize, Debug)]
    struct Query {
        #[serde(default = "default_protocol_version")]
        protocol_version: u16,

        #[serde(default = "default_protocol_version")]
        min_protocol_version: u16,
    }

    let query: Query = serde_urlencoded::from_str(req.uri().query().unwrap_or(""))?;

    let lock = ctx.gameservers.read().unwrap();
    let value = lock
        .iter()
        .filter(|(_, (_, info))| {
            info.min_protocol_version <= query.protocol_version
                && info.protocol_version >= query.min_protocol_version
        })
        .choose(&mut rand::thread_rng())
        .ok_or(Error::InternalStrStatic("no available servers"))?;
    let value = &value.1 .1;

    let info = ni_ty::protocol::ServerConnectionInfo {
        server_id: value.server_id,
        address_ipv4: value.address_ipv4,
        hostname: value.hostname.as_deref().map(Cow::Borrowed),
        web_port: value.web_port,
    };

    json_response(&info)
}

async fn handler_servers_get(
    params: (u8,),
    ctx: Arc<GlobalState>,
    _req: hyper::Request<hyper::Body>,
) -> Result<hyper::Response<hyper::Body>, crate::Error> {
    let (server_id,) = params;

    if let Some(value) = ctx.gameservers.read().unwrap().get(&server_id) {
        let value = &value.1;

        let info = ni_ty::protocol::ServerConnectionInfo {
            server_id: value.server_id,
            address_ipv4: value.address_ipv4,
            hostname: value.hostname.as_deref().map(Cow::Borrowed),
            web_port: value.web_port,
        };

        json_response(&info)
    } else {
        Ok(simple_response(
            hyper::StatusCode::NOT_FOUND,
            "No such server",
        ))
    }
}

async fn handler_stats_get(
    _: (),
    ctx: Arc<GlobalState>,
    _req: hyper::Request<hyper::Body>,
) -> Result<hyper::Response<hyper::Body>, crate::Error> {
    let stats = ctx.gameservers.read().unwrap().iter().fold(
        ni_ty::protocol::ServerStats {
            public_games: 0,
            private_games: 0,
            public_game_players: 0,
            private_game_players: 0,
        },
        |mut acc, entry| {
            let stats = &entry.1 .1.stats;
            acc.public_games += stats.public_games;
            acc.private_games += stats.private_games;
            acc.public_game_players += stats.public_game_players;
            acc.private_game_players += stats.private_game_players;

            acc
        },
    );

    json_response(&stats)
}

#[tokio::main]
async fn main() {
    let addr = ([0, 0, 0, 0], 6462).into();

    let routes: Arc<RouteNode<()>> = Arc::new(
        RouteNode::new()
            .with_child(
                "public_games",
                RouteNode::new().with_handler_async("GET", handler_public_games_list),
            )
            .with_child(
                "servers:pick_for_new_game",
                RouteNode::new().with_handler_async("POST", handler_servers_pick_for_new_game),
            )
            .with_child(
                "servers",
                RouteNode::new().with_child_parse::<u8, _>(
                    RouteNode::new().with_handler_async("GET", handler_servers_get),
                ),
            )
            .with_child(
                "stats",
                RouteNode::new().with_handler_async("GET", handler_stats_get),
            ),
    );

    let global_state = Arc::new(GlobalState {
        gameservers: Default::default(),
    });

    let server = hyper::Server::bind(&addr).serve({
        let global_state = global_state.clone();
        hyper::service::make_service_fn(move |_conn| {
            let global_state = global_state.clone();
            let routes = routes.clone();

            futures_util::future::ok::<_, std::convert::Infallible>(hyper::service::service_fn(
                move |req| {
                    let global_state = global_state.clone();
                    let routes = routes.clone();

                    async move {
                        let result = match routes.route(req, global_state) {
                            Ok(fut) => fut.await,
                            Err(err) => Err(Error::RoutingError(err)),
                        };

                        Ok::<_, hyper::Error>(match result {
                            Ok(val) => val,
                            Err(Error::UserError(res)) => res,
                            Err(Error::RoutingError(err)) => {
                                let code = match err {
                                    trout::RoutingFailure::NotFound => hyper::StatusCode::NOT_FOUND,
                                    trout::RoutingFailure::MethodNotAllowed => {
                                        hyper::StatusCode::METHOD_NOT_ALLOWED
                                    }
                                };

                                simple_response(code, code.canonical_reason().unwrap())
                            }
                            Err(Error::Internal(err)) => {
                                eprintln!("Error: {:?}", err);

                                simple_response(
                                    hyper::StatusCode::INTERNAL_SERVER_ERROR,
                                    "Internal Server Error",
                                )
                            }
                            Err(Error::InternalStr(err)) => {
                                eprintln!("Error: {}", err);

                                simple_response(
                                    hyper::StatusCode::INTERNAL_SERVER_ERROR,
                                    "Internal Server Error",
                                )
                            }
                            Err(Error::InternalStrStatic(err)) => {
                                eprintln!("Error: {}", err);

                                simple_response(
                                    hyper::StatusCode::INTERNAL_SERVER_ERROR,
                                    "Internal Server Error",
                                )
                            }
                        })
                    }
                },
            ))
        })
    });

    let redis_conn = nertsio_server_common::redis_connection_builder_from_uri(
        &std::env::var("REDIS_URI").expect("Missing REDIS_URI"),
    )
    .pubsub_connect()
    .await
    .expect("Failed to connect to Redis");
    let sub_stream = redis_conn
        .subscribe(ni_ty::protocol::COORDINATOR_CHANNEL)
        .await
        .expect("Failed to subscribe to channel");

    if let Err(err) = futures_util::try_join!(
        server.map_err(Into::into),
        {
            let global_state = global_state.clone();
            async move {
                sub_stream
                    .try_for_each(|value| {
                        if let redis_async::resp::RespValue::BulkString(bytes) = value {
                            match serde_json::from_slice::<ni_ty::protocol::ServerStatusMessage>(
                                &bytes,
                            ) {
                                Ok(message) => {
                                    println!("message = {:?}", message);

                                    global_state.gameservers.write().unwrap().insert(
                                        message.server_id,
                                        (std::time::Instant::now(), message),
                                    );
                                }
                                Err(err) => {
                                    eprintln!("failed to parse message: {:?}", err);
                                }
                            }
                        } else {
                            eprintln!("received unknown message {:?}", value);
                        }

                        futures_util::future::ok(())
                    })
                    .await?;
                Result::<(), _>::Err(anyhow::anyhow!("subscription stream ended"))
            }
        },
        async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(2));

            loop {
                interval.tick().await;

                global_state
                    .gameservers
                    .write()
                    .unwrap()
                    .retain(|_key, value| value.0.elapsed() < GAMESERVER_TIMEOUT);
            }

            // helps infer return type
            #[allow(unreachable_code)]
            Ok(())
        }
    ) {
        eprintln!("Error: {:?}", err);
    }
}
