use std::sync::Arc;

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

struct RouteContext {}

type RouteNode<P> = trout::Node<
    P,
    hyper::Request<hyper::Body>,
    std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<hyper::Response<hyper::Body>, Error>> + Send>,
    >,
    Arc<RouteContext>,
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

#[tokio::main]
async fn main() {
    let addr = ([0, 0, 0, 0], 6462).into();

    let routes: Arc<RouteNode<()>> = Arc::new(RouteNode::new());

    let context = Arc::new(RouteContext {});

    let server = hyper::Server::bind(&addr).serve(hyper::service::make_service_fn(move |_conn| {
        let context = context.clone();
        let routes = routes.clone();

        futures_util::future::ok::<_, std::convert::Infallible>(hyper::service::service_fn(
            move |req| {
                let context = context.clone();
                let routes = routes.clone();

                async move {
                    let result = match routes.route(req, context) {
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
    }));

    if let Err(err) = server.await {
        eprintln!("Failed to run server: {:?}", err);
    }
}
