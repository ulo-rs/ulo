//! The standalone server's `prepare` report lists its failures in one order on every run: the
//! server's two timeouts, then what the gateway table refuses (the server's gateway defaults, its
//! gateways, the absence of any `port = own` one), then its endpoints. A zero server-wide
//! in-flight bound is among the table's refusals, and so is a zero `max_message`, whose refusal
//! names the 64 MiB default an unset one leaves.

use ulo::{App, Bound, Module, ModuleDef, ModuleIdentity, Signal, StartupError};
use ulo_transport::Count;

/// Declares no gateway, so the table refuses a server with none.
struct Empty;

impl Module for Empty {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        let _ = m;
    }
}

#[tokio::test]
async fn the_server_s_timeouts_come_before_the_table_s_failures_and_the_endpoints_after() {
    let zero = Bound::After(std::time::Duration::ZERO);
    let server = ulo_ws_hyper::Server::new("127.0.0.1:0")
        .endpoint("not an endpoint")
        .header_timeout(zero)
        .handshake_timeout(zero)
        .max_inflight(Count::Max(0));
    let bound = App::builder(Empty)
        .runtime(ulo_runtime_tokio::Tokio::current())
        .wire()
        .expect("the app wires")
        .connect()
        .await
        .expect("the app connects")
        .bind(server)
        .listen()
        .await;
    let text = match bound {
        Err(StartupError::Configure(refused)) => refused.to_string(),
        Err(other) => panic!("expected a `Configure` refusal, got: {other}"),
        Ok(app) => {
            let _ = app.handle().close(Signal::new("prepare")).await;
            panic!("a server with every failure listed listened");
        }
    };
    let expected = [
        "`.header_timeout(Bound::After(Duration::ZERO))`",
        "`.handshake_timeout(Bound::After(Duration::ZERO))`",
        "ulo_ws_hyper::Server: `.max_inflight(Count::Max(0))`",
        "`ulo_ws_hyper::Server` serves the gateways declared `port = own`, and none is",
        "not an endpoint",
    ];
    let at: Vec<Option<usize>> = expected.iter().map(|failure| text.find(failure)).collect();
    assert!(at.iter().all(Option::is_some), "a failure is missing from the report, {expected:?} at {at:?}:\n{text}");
    assert!(at.is_sorted(), "the report lists its failures out of order, {expected:?} at {at:?}:\n{text}");
}

#[tokio::test]
async fn a_zero_server_wide_in_flight_bound_is_refused() {
    let server = ulo_ws_hyper::Server::new("127.0.0.1:0").server_max_inflight(Count::Max(0));
    let bound = App::builder(Empty)
        .runtime(ulo_runtime_tokio::Tokio::current())
        .wire()
        .expect("the app wires")
        .connect()
        .await
        .expect("the app connects")
        .bind(server)
        .listen()
        .await;
    let text = match bound {
        Err(StartupError::Configure(refused)) => refused.to_string(),
        Err(other) => panic!("expected a `Configure` refusal, got: {other}"),
        Ok(app) => {
            let _ = app.handle().close(Signal::new("prepare")).await;
            panic!("a server bounding its connections at zero messages in flight listened");
        }
    };
    assert!(text.contains("ulo_ws_hyper::Server: `.server_max_inflight(Count::Max(0))`"), "{text}");
}

#[tokio::test]
async fn a_zero_max_message_is_refused_naming_the_default_unset_leaves() {
    let server = ulo_ws_hyper::Server::new("127.0.0.1:0").max_message(0);
    let bound = App::builder(Empty)
        .runtime(ulo_runtime_tokio::Tokio::current())
        .wire()
        .expect("the app wires")
        .connect()
        .await
        .expect("the app connects")
        .bind(server)
        .listen()
        .await;
    let text = match bound {
        Err(StartupError::Configure(refused)) => refused.to_string(),
        Err(other) => panic!("expected a `Configure` refusal, got: {other}"),
        Ok(app) => {
            let _ = app.handle().close(Signal::new("prepare")).await;
            panic!("a server closing every connection at its first message listened");
        }
    };
    let hint = "ulo_ws_hyper::Server: `.max_message(0)` would close every connection with 1009 at its first message; \
                leave it unset for the 64 MiB default";
    assert!(text.contains(hint), "{text}");
}
