//! A broker refusing the first connect fails the start as `Bind`, carrying the broker's error, in
//! the time the refusal takes: the first connection is made without lapin's auto-recovery, which
//! would retry it sixteen times on a backoff past 30 s. The address is a port nothing listens on,
//! so no broker is needed.

use std::error::Error;
use std::io::ErrorKind;
use std::net::TcpListener;
use std::time::{Duration, Instant};

use ulo::{App, Module, ModuleDef, ModuleIdentity, Signal, StartupError};
use ulo_rpc::Server;
use ulo_rpc_rabbitmq::RabbitMq;
use ulo_runtime_tokio::Tokio;

struct Root;

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        let _ = m;
    }
}

/// A loopback port that refuses connections: bound, read and released.
fn refusing_port() -> u16 {
    TcpListener::bind("127.0.0.1:0").expect("a loopback port").local_addr().expect("the port's address").port()
}

/// How long a refusal may take to report; a refused loopback connect answers in milliseconds.
const PROMPT: Duration = Duration::from_secs(2);

#[tokio::test(flavor = "multi_thread")]
async fn a_broker_refusing_the_first_connect_fails_the_start_as_bind_at_once() {
    let app = App::builder(Root).runtime(Tokio::current()).wire().expect("the app wires").connect().await.expect("the app connects");
    let serving = RabbitMq::url(format!("amqp://guest:guest@127.0.0.1:{}/%2f", refusing_port()));
    let started = Instant::now();
    // A hang is the failure: the bound is a watchdog, not a timing the link keeps.
    let listened = tokio::time::timeout(Duration::from_secs(20), app.bind(Server::new(serving)).listen()).await;
    let elapsed = started.elapsed();
    let error = match listened {
        Err(_) => panic!("`listen` against a refusing port did not return within the watchdog bound"),
        Ok(Ok(app)) => {
            let _ = app.handle().close(Signal::new("refused")).await;
            panic!("a server listened against a port that refuses connections");
        }
        Ok(Err(error)) => error,
    };
    assert!(elapsed < PROMPT, "the refusal took {elapsed:?}, past {PROMPT:?}");
    assert!(!ulo::is_permanent(&error), "a refused broker is marked permanent, though a later start may reach it: {error}");
    let StartupError::Bind { source, .. } = error else { panic!("expected a `Bind` failure, got: {error}") };
    let cause = source.into_inner();
    let mut link: Option<&(dyn Error + 'static)> = Some(&*cause);
    let refused = std::iter::from_fn(|| {
        let current = link?;
        link = current.source();
        Some(current)
    })
    .filter_map(|error| error.downcast_ref::<std::io::Error>())
    .any(|io| io.kind() == ErrorKind::ConnectionRefused);
    assert!(refused, "the `Bind` does not carry the broker's `ConnectionRefused`: {cause}");
}
