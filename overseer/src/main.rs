use nertsio_types as ni_ty;
use redis::FromRedisValue;
use std::sync::{Arc, RwLock};

struct GlobalState {
    gameservers: RwLock<indexmap::IndexMap<u8, ServerState>>,
}

struct ServerState {
    last_updated: std::time::Instant,
    message: ni_ty::protocol::ServerStatusMessage<'static>,
    ping_result: Option<bool>,
}

#[tokio::main]
async fn main() {
    let global_state = Arc::new(GlobalState {
        gameservers: Default::default(),
    });

    let metrics_registry = prometheus::Registry::new();
    {
        let failed_pings_gauge = prometheus::PullingGauge::new(
            "failed_pings",
            "Number of ping checks that have failed.",
            {
                let global_state = global_state.clone();
                Box::new(move || {
                    global_state
                        .gameservers
                        .read()
                        .unwrap()
                        .values()
                        .filter(|x| x.ping_result == Some(false))
                        .count() as f64
                })
            },
        )
        .unwrap();
        metrics_registry
            .register(Box::new(failed_pings_gauge))
            .unwrap();
    }

    let metrics_port: u16 = std::env::var("METRICS_PORT")
        .expect("Missing METRICS_PORT")
        .parse()
        .expect("Invalid value for METRICS_PORT");

    let metrics_server = hyper::Server::bind(&([0, 0, 0, 0], metrics_port).into()).serve({
        let registry = metrics_registry.clone();
        hyper::service::make_service_fn(move |_conn: &hyper::server::conn::AddrStream| {
            let registry = registry.clone();

            futures_util::future::ok::<_, std::convert::Infallible>(hyper::service::service_fn(
                move |_req| {
                    let registry = registry.clone();
                    async move {
                        let body = prometheus::TextEncoder.encode_to_string(&registry.gather())?;

                        let mut res = hyper::Response::new(body);
                        res.headers_mut().insert(
                            hyper::header::CONTENT_TYPE,
                            hyper::header::HeaderValue::from_static("text/plain; version=0.0.4"),
                        );

                        Ok::<_, anyhow::Error>(res)
                    }
                },
            ))
        })
    });

    let (redis_msg_tx, mut redis_msg_rx) = tokio::sync::mpsc::unbounded_channel();

    let mut redis_conn = redis::Client::open({
        use redis::IntoConnectionInfo;

        let mut info = std::env::var("REDIS_URI")
            .expect("Missing REDIS_URI")
            .into_connection_info()
            .expect("Failed to parse REDIS_URI");

        info.redis.protocol = redis::ProtocolVersion::RESP3;

        info
    })
    .expect("Failed to connect to Redis")
    .get_multiplexed_async_connection_with_config(
        &redis::AsyncConnectionConfig::new().set_push_sender(redis_msg_tx),
    )
    .await
    .expect("Failed to connect to Redis");

    redis_conn
        .subscribe(ni_ty::protocol::COORDINATOR_CHANNEL)
        .await
        .expect("Failed to subscribe to channel");

    if let Err(err) = futures_util::try_join!(
        async move {
            metrics_server.await?;

            Ok(())
        },
        {
            let global_state = global_state.clone();
            async move {
                while let Some(msg) = redis_msg_rx.recv().await {
                    if msg.kind != redis::PushKind::Message {
                        continue;
                    }

                    match String::from_owned_redis_value(
                        msg.data.into_iter().skip(1).next().unwrap(),
                    ) {
                        Ok(content) => {
                            match serde_json::from_str::<ni_ty::protocol::ServerStatusMessage>(
                                &content,
                            ) {
                                Ok(message) => {
                                    let mut gameservers = global_state.gameservers.write().unwrap();

                                    match gameservers.entry(message.server_id) {
                                        indexmap::map::Entry::Occupied(mut entry) => {
                                            entry.get_mut().last_updated =
                                                std::time::Instant::now();
                                            entry.get_mut().message = message;
                                        }
                                        indexmap::map::Entry::Vacant(entry) => {
                                            entry.insert(ServerState {
                                                last_updated: std::time::Instant::now(),
                                                message,
                                                ping_result: None,
                                            });
                                        }
                                    }
                                }
                                Err(err) => {
                                    eprintln!("failed to parse message: {:?}", err);
                                }
                            }
                        }
                        Err(err) => eprintln!("unexpected type for message: {:?}", err),
                    }
                }

                Result::<(), _>::Err(anyhow::anyhow!("subscription stream ended"))
            }
        },
        {
            let global_state = global_state.clone();

            async move {
                let mut interval = tokio::time::interval(std::time::Duration::from_secs(2));

                loop {
                    interval.tick().await;

                    global_state
                        .gameservers
                        .write()
                        .unwrap()
                        .retain(|_key, value| {
                            value.last_updated.elapsed()
                                < nertsio_common::GAMESERVER_PUBLISH_TIMEOUT
                        });
                }

                // helps infer return type
                #[allow(unreachable_code)]
                Ok(())
            }
        },
        async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));

            let mut i = 0;

            loop {
                interval.tick().await;

                let target = {
                    let gameservers = global_state.gameservers.read().unwrap();
                    if i >= gameservers.len() {
                        i = 0;
                    }

                    if gameservers.len() < 1 {
                        continue; // no servers to check
                    }

                    gameservers.get_index(i).unwrap().1.message.clone()
                };

                let result = run_test(&target).await;
                println!("Test result for {}: {:?}", target.server_id, result);

                {
                    if let Some(state) = global_state
                        .gameservers
                        .write()
                        .unwrap()
                        .get_mut(&target.server_id)
                    {
                        state.ping_result = Some(result.is_ok());
                    }
                }

                i += 1;
            }

            // helps infer return type
            #[allow(unreachable_code)]
            Ok(())
        },
    ) {
        eprintln!("Error: {:?}", err);
    }
}

async fn run_test(
    target: &ni_ty::protocol::ServerStatusMessage<'static>,
) -> Result<(), anyhow::Error> {
    tokio::time::timeout(std::time::Duration::from_secs(7), run_test_inner(target)).await??;
    Ok(())
}

async fn run_test_inner(
    target: &ni_ty::protocol::ServerStatusMessage<'static>,
) -> Result<(), anyhow::Error> {
    wtransport::Endpoint::client(
        wtransport::ClientConfig::builder()
            .with_bind_default()
            .with_no_cert_validation()
            .build(),
    )?
    .connect(&format!(
        "https://{}:{}",
        target
            .hostname
            .as_ref()
            .ok_or(anyhow::anyhow!("No hostname for server"))?,
        target
            .web_port
            .as_ref()
            .ok_or(anyhow::anyhow!("No web_port for server"))?
    ))
    .await?;

    Ok(())
}
