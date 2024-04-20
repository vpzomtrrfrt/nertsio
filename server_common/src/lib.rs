pub fn redis_connection_builder_from_uri(src: &str) -> redis_async::client::ConnectionBuilder {
    let src = iref::Uri::new(src).expect("Invalid value for REDIS_URI");
    if src.scheme().as_str() != "redis" {
        panic!("Unsupported scheme for REDIS_URI");
    }

    let authority = src.authority().expect("Missing host in REDIS_URI");

    let host = authority.host().as_str();
    let port = match authority.port() {
        Some(port) => port.as_str().parse().unwrap(),
        None => 6379,
    };

    let (username, password) = match authority.user_info() {
        Some(user_info) => {
            let user_info = user_info.as_str();
            match user_info.find('@') {
                Some(idx) => (Some(&user_info[..idx]), Some(&user_info[(idx + 1)..])),
                None => (None, Some(user_info)),
            }
        }
        None => (None, None),
    };

    let mut result = redis_async::client::ConnectionBuilder::new(host, port).unwrap();

    if let Some(username) = username {
        result.username(username);
    }

    if let Some(password) = password {
        result.password(password);
    }

    result
}
