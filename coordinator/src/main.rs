use futures_util::{StreamExt, TryFutureExt};
use geo::Distance;
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

    regions: HashMap<String, RegionConfig>,

    geoip_db: Option<geoip2::Reader<'static, geoip2::City<'static>>>,
}

impl GlobalState {
    pub fn get_region_for_output<'a>(&'a self, id: &'a str) -> ni_ty::RegionInfo {
        match self.regions.get(id) {
            None => ni_ty::RegionInfo {
                id: id.into(),
                name: id.into(),
            },
            Some(info) => ni_ty::RegionInfo {
                id: (&info.id).into(),
                name: (&info.name).into(),
            },
        }
    }
}

#[derive(Deserialize)]
struct RegionConfig {
    id: String,
    name: String,
    lat: f64,
    lon: f64,
}

struct Request {
    request: hyper::Request<hyper::Body>,
    ip_address: std::net::IpAddr,
}

impl AsRef<hyper::Request<hyper::Body>> for Request {
    fn as_ref(&self) -> &hyper::Request<hyper::Body> {
        &self.request
    }
}

impl trout::Request for Request {
    fn path(&self) -> &str {
        self.request.path()
    }

    fn method(&self) -> &str {
        self.request.method().as_str()
    }
}

type RouteNode<P> = trout::Node<
    P,
    Request,
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
    _req: Request,
) -> Result<hyper::Response<hyper::Body>, crate::Error> {
    use rand::seq::IteratorRandom;

    let gameservers = ctx.gameservers.read().unwrap();

    let list_games = gameservers
        .iter()
        .flat_map(|(server_id, (_, info))| {
            let server_address_ipv4 = info.address_ipv4;
            let server_hostname = info.hostname.as_deref();
            let server_web_port = info.web_port;
            let server_region = info
                .region
                .as_deref()
                .map(|id| ctx.get_region_for_output(id));
            info.open_public_games
                .iter()
                .map(move |game| ni_ty::protocol::PublicGameInfoExpanded {
                    game_id: game.game_id,
                    players: game.players,
                    real_players: game.real_players,
                    active_players: game.active_players,
                    max_players: game.max_players,
                    waiting: game.waiting,
                    #[allow(deprecated)]
                    server: ni_ty::protocol::ServerConnectionInfo {
                        server_id: *server_id,
                        address_ipv4: server_address_ipv4,
                        hostname: server_hostname.map(Cow::Borrowed),
                        web_port: server_web_port,
                        region: server_region.clone(),
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
    req: Request,
) -> Result<hyper::Response<hyper::Body>, crate::Error> {
    #[derive(Deserialize, Debug)]
    struct Query {
        #[serde(default = "default_protocol_version")]
        protocol_version: u16,

        #[serde(default = "default_protocol_version")]
        min_protocol_version: u16,
    }

    let query: Query = serde_urlencoded::from_str(req.as_ref().uri().query().unwrap_or(""))?;

    let user_loc = ctx
        .geoip_db
        .as_ref()
        .and_then(|geoip_db| {
            let res = geoip_db.lookup(req.ip_address);
            if let Err(ref err) = res {
                println!("Failed to look up IP address location: {:?}", err);
            }

            res.ok()
        })
        .and_then(|city| {
            println!("city = {:?}", city);

            city.location
        })
        .and_then(|location| match (location.latitude, location.longitude) {
            (Some(lat), Some(lon)) => Some(geo::Point::new(lat, lon)),
            _ => None,
        });

    let region_priority: Vec<&String> = match user_loc {
        None => ctx.regions.keys().collect(),
        Some(user_loc) => {
            let mut result: Vec<_> = ctx
                .regions
                .values()
                .map(|region| {
                    let loc = geo::Point::new(region.lat, region.lon);
                    (geo::Haversine::distance(loc, user_loc), &region.id)
                })
                .collect();
            result.sort_unstable_by(|(a, _), (b, _)| a.total_cmp(b));
            result.into_iter().map(|(_, id)| id).collect()
        }
    };

    println!("region_priority = {:?}", region_priority);

    let lock = ctx.gameservers.read().unwrap();
    let options: Vec<_> = lock
        .iter()
        .filter(|(_, (_, info))| {
            info.min_protocol_version <= query.protocol_version
                && info.protocol_version >= query.min_protocol_version
        })
        .collect();

    if options.is_empty() {
        return Err(Error::InternalStrStatic("no available servers"));
    }

    // eventually should make this less deterministic, but for now we don't have enough traffic
    // that it matters

    let best = &options.first().as_ref().unwrap().1 .1;
    let mut best = (
        region_priority
            .iter()
            .position(|x| Some(Cow::Borrowed(x.as_ref())) == best.region)
            .unwrap_or(usize::MAX),
        best,
    );

    for i in 1..options.len() {
        let current = &options[i].1 .1;

        let priority = region_priority
            .iter()
            .position(|x| Some(Cow::Borrowed(x.as_ref())) == current.region)
            .unwrap_or(usize::MAX);
        if priority < best.0 {
            best = (priority, current);
        }
    }

    let value = best.1;

    #[allow(deprecated)]
    let info = ni_ty::protocol::ServerConnectionInfo {
        server_id: value.server_id,
        address_ipv4: value.address_ipv4,
        hostname: value.hostname.as_deref().map(Cow::Borrowed),
        web_port: value.web_port,
        region: value
            .region
            .as_deref()
            .map(|id| ctx.get_region_for_output(id)),
    };

    json_response(&info)
}

async fn handler_servers_get(
    params: (u8,),
    ctx: Arc<GlobalState>,
    _req: Request,
) -> Result<hyper::Response<hyper::Body>, crate::Error> {
    let (server_id,) = params;

    if let Some(value) = ctx.gameservers.read().unwrap().get(&server_id) {
        let value = &value.1;

        #[allow(deprecated)]
        let info = ni_ty::protocol::ServerConnectionInfo {
            server_id: value.server_id,
            address_ipv4: value.address_ipv4,
            hostname: value.hostname.as_deref().map(Cow::Borrowed),
            web_port: value.web_port,
            region: value
                .region
                .as_deref()
                .map(|id| ctx.get_region_for_output(id)),
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
    _req: Request,
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

    let regions: HashMap<String, RegionConfig> = match std::env::var("REGIONS_CONFIG") {
        Ok(value) => {
            let list: Vec<RegionConfig> =
                serde_json::from_str(&value).expect("Failed to parse REGIONS_CONFIG");

            list.into_iter()
                .map(|config| (config.id.clone(), config))
                .collect()
        }
        Err(std::env::VarError::NotPresent) => Default::default(),
        Err(std::env::VarError::NotUnicode(_)) => panic!("REGIONS_CONFIG is not valid unicode"),
    };

    let geoip_db = std::env::var_os("GEOIP_DB_FILE").map(|path| {
        let mut content = std::fs::read(path).expect("Failed to read geoip db");
        content.shrink_to_fit();

        // leak this because it will live for the entire runtime and otherwise we would have
        // self-referencing structures to deal with
        let content = content.leak();

        geoip2::Reader::<geoip2::City>::from_bytes(content).expect("Failed to read geoip db")
    });

    let global_state = Arc::new(GlobalState {
        gameservers: Default::default(),
        regions,
        geoip_db,
    });

    let server = hyper::Server::bind(&addr).serve({
        let global_state = global_state.clone();
        hyper::service::make_service_fn(move |conn: &hyper::server::conn::AddrStream| {
            let global_state = global_state.clone();
            let routes = routes.clone();

            let raw_ip = conn.remote_addr().ip();

            futures_util::future::ok::<_, std::convert::Infallible>(hyper::service::service_fn(
                move |req| {
                    let global_state = global_state.clone();
                    let routes = routes.clone();

                    let forwarded_ip: Option<std::net::IpAddr> = req
                        .headers()
                        .get("x-forwarded-for")
                        .and_then(|value| match value.to_str() {
                            Err(_) => None,
                            Ok(value) => {
                                let value = match value.find(',') {
                                    None => value,
                                    Some(idx) => &value[..idx],
                                };

                                value.parse().ok()
                            }
                        });

                    let ip = forwarded_ip.unwrap_or(raw_ip);

                    let req = Request {
                        request: req,
                        ip_address: ip,
                    };

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

    let mut redis_conn =
        redis::Client::open(std::env::var("REDIS_URI").expect("Missing REDIS_URI"))
            .expect("Failed to connect to Redis")
            .get_async_pubsub()
            .await
            .expect("Failed to connect to Redis");

    redis_conn
        .subscribe(ni_ty::protocol::COORDINATOR_CHANNEL)
        .await
        .expect("Failed to subscribe to channel");

    if let Err(err) = futures_util::try_join!(
        server.map_err(Into::into),
        {
            let global_state = global_state.clone();
            async move {
                redis_conn
                    .on_message()
                    .for_each(|value| {
                        match value.get_payload::<String>() {
                            Ok(content) => {
                                match serde_json::from_str::<ni_ty::protocol::ServerStatusMessage>(
                                    &content,
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
                            }
                            Err(err) => eprintln!("failed to parse message: {:?}", err),
                        }

                        futures_util::future::ready(())
                    })
                    .await;
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
