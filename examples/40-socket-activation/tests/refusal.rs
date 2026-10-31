//! Adopting a socket the process was not handed is a startup refusal that names the socket.
//! This test process inherited none: cargo sets no `LISTEN_PID`. Unix only, since elsewhere every
//! inherited endpoint is refused as unsupported.
#![cfg(unix)]

use ulo::StartupError;
use ulo_net::Endpoint;
use ulo_runtime_tokio::Tokio;

use ex_40_socket_activation::{builder, server};

async fn listen_on(server: ulo_http_hyper::Server) -> StartupError {
    let bound = builder().runtime(Tokio::current()).wire().unwrap().connect().await.unwrap().bind(server).listen().await;
    match bound {
        Err(error) => error,
        Ok(_) => panic!("listen adopted a socket nobody handed over"),
    }
}

#[tokio::test]
async fn an_inherited_socket_nobody_handed_over_is_refused_naming_its_index() {
    let error = listen_on(server(true, "0")).await;
    assert!(matches!(error, StartupError::Configure(_)), "{error:?}");
    assert!(error.to_string().contains("no inherited socket #0"), "{error}");
}

#[tokio::test]
async fn an_inherited_name_nobody_handed_over_is_refused_naming_it() {
    let error = listen_on(ulo_http_hyper::Server::new(Endpoint::inherited("web"))).await;
    assert!(matches!(error, StartupError::Configure(_)), "{error:?}");
    assert!(error.to_string().contains("no inherited socket is named `web`"), "{error}");
}
