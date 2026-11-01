//! A broadcast adapter whose subscription fails fails the HTTP server's `listen()` through the
//! hand-off, as `Bind` carrying the adapter's error: the server's port is released, and no
//! gateway's `AfterInit` has run.

use std::net::TcpListener as StdListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use bytes::Bytes;
use futures_util::stream::BoxStream;
use ulo::{App, BoxError, BoxFuture, Dep, Module, ModuleDef, ModuleIdentity, Signal, StartupError, Timer, injectable, routes};
use ulo_ws::{AfterInit, BroadcastAdapter, GatewayRef, NodeId, Target, WsModule};

const REFUSAL: &str = "the adapter's subscription was refused";

/// Publishes nothing anywhere, and fails every subscription.
struct Unsubscribable;

impl BroadcastAdapter for Unsubscribable {
    fn prepare(&self) -> Result<(), BoxError> {
        Ok(())
    }

    fn publish(&self, _target: Target, _frame: Bytes) -> BoxFuture<'static, Result<(), BoxError>> {
        Box::pin(async { Ok(()) })
    }

    fn subscribe(&self, _node: NodeId, _timer: Arc<dyn Timer>) -> BoxFuture<'static, Result<BoxStream<'static, (Target, Bytes)>, BoxError>> {
        Box::pin(async { Err(BoxError::from(REFUSAL)) })
    }
}

/// How many times a gateway's `AfterInit` ran.
#[derive(Clone, Default)]
struct Inits(Arc<AtomicUsize>);

#[injectable]
struct Lobby {
    inits: Dep<Inits>,
}

#[routes]
#[ulo_ws::gateway(path = "/lobby")]
impl Lobby {
    #[ulo_ws::message("ping")]
    fn ping(&self) -> &'static str {
        "pong"
    }
}

impl AfterInit for Lobby {
    async fn after_init(&self, gw: GatewayRef) {
        let _ = gw;
        self.inits.0.fetch_add(1, Ordering::SeqCst);
    }
}

struct Root {
    inits: Inits,
}

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.import(WsModule::for_root().broadcast(Unsubscribable));
        m.value(self.inits.clone());
        m.controller::<Lobby>();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_subscription_fails_the_http_server_s_listen_as_bind_and_releases_its_port() {
    // Bound, read and released, so the server binds a port the test knows.
    let addr = StdListener::bind("127.0.0.1:0").expect("a loopback port").local_addr().expect("its address");
    let inits = Inits::default();
    let listened = App::builder(Root { inits: inits.clone() })
        .runtime(ulo_runtime_tokio::Tokio::current())
        .wire()
        .expect("the app wires")
        .connect()
        .await
        .expect("the app connects")
        .bind(ulo_http_hyper::Server::new(&*addr.to_string()))
        .listen()
        .await;
    let error = match listened {
        Ok(app) => {
            let _ = app.handle().close(Signal::new("subscribe_refused")).await;
            panic!("the HTTP server listened though the broadcast adapter could not subscribe");
        }
        Err(error) => error,
    };
    let StartupError::Bind { source, .. } = &error else { panic!("expected `Bind`, got: {error}") };
    assert!(source.to_string().contains(REFUSAL), "the `Bind` does not carry the adapter's error: {source}");
    assert!(StdListener::bind(addr).is_ok(), "the HTTP server still holds {addr} after its bind failed");
    assert_eq!(inits.0.load(Ordering::SeqCst), 0, "a gateway's `AfterInit` ran though the subscription failed");
}
