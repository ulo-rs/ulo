//! Q29-9: a pattern whose exchange or queue name is over AMQP's 255 bytes fails the start before
//! the server's link connects, as a permanent `Bind` naming the pattern, its length and the
//! limit; a pattern at the limit goes on to connect. Patterns of 241 bytes, and of 208 for an event
//! declared `every_instance`, are the longest served. No broker is needed: the refused start
//! leaves a listener the test holds unconnected to, and the served one reaches a port that
//! refuses connections.

use std::error::Error;
use std::io::ErrorKind;
use std::net::TcpListener;

use ulo::{App, BoxError, Construct, ConstructError, Controller, Dependencies, HandlerSpec, Handlers, Module, ModuleDef, ModuleIdentity};
use ulo::{Mount, Resolver, Signal, StartupError};
use ulo_rpc::{Reply, Rpc, RpcCx, RpcHandler, Server};
use ulo_rpc_rabbitmq::RabbitMq;
use ulo_runtime_tokio::Tokio;

/// One handler on a pattern of `BYTES` bytes, an event declared `every_instance` when `EVERY`
/// and a request otherwise.
struct Long<const BYTES: usize, const EVERY: bool>;

impl<const BYTES: usize, const EVERY: bool> Construct for Long<BYTES, EVERY> {
    type Scope = ulo::scope::Auto;

    fn dependencies(d: &mut Dependencies) {
        d.mark::<Self>();
    }

    async fn construct(_: &Resolver<'_>) -> Result<Self, ConstructError> {
        Ok(Long)
    }
}

async fn answer(_cx: RpcCx) -> Result<Reply, BoxError> {
    Ok(Reply::None)
}

impl<const BYTES: usize, const EVERY: bool> Controller for Long<BYTES, EVERY> {}

impl<const BYTES: usize, const EVERY: bool> Handlers for Long<BYTES, EVERY> {
    fn mount(m: &mut Mount<'_>) {
        let handler = if EVERY { RpcHandler::event(answer).every_instance() } else { RpcHandler::message(answer) };
        m.handler(HandlerSpec::<Rpc, _>::new("answer", handler).route(pattern(BYTES)));
    }
}

fn pattern(bytes: usize) -> String {
    "p".repeat(bytes)
}

struct Root<const BYTES: usize, const EVERY: bool>;

impl<const BYTES: usize, const EVERY: bool> Module for Root<BYTES, EVERY> {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.controller::<Long<BYTES, EVERY>>();
    }
}

/// The start of a server whose one handler is `Root`'s, on a link to `port`.
async fn started<const BYTES: usize, const EVERY: bool>(port: u16) -> StartupError {
    let app = App::builder(Root::<BYTES, EVERY>).runtime(Tokio::current()).wire().expect("the app wires").connect().await.expect("the app connects");
    match app.bind(Server::new(RabbitMq::url(format!("amqp://guest:guest@127.0.0.1:{port}/%2f")))).listen().await {
        Ok(app) => {
            let _ = app.handle().close(Signal::new("listened")).await;
            panic!("a server listened on a {BYTES}-byte pattern with no broker");
        }
        Err(error) => error,
    }
}

/// A start refused for a pattern's names: before any connection to the listener the link was
/// pointed at, as a permanent `Bind` whose text holds `parts`.
async fn refused<const BYTES: usize, const EVERY: bool>(parts: &[String]) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
    listener.set_nonblocking(true).expect("the listener goes non-blocking");
    let error = started::<BYTES, EVERY>(listener.local_addr().expect("the listener's address").port()).await;
    assert!(
        matches!(listener.accept(), Err(error) if error.kind() == ErrorKind::WouldBlock),
        "the link connected before refusing a {BYTES}-byte pattern"
    );
    assert!(ulo::is_permanent(&error), "a pattern AMQP cannot name is refused as retryable: {error}");
    let StartupError::Bind { source, .. } = error else { panic!("expected a `Bind` failure, got: {error}") };
    let text = source.into_inner().to_string();
    for part in parts {
        assert!(text.contains(part.as_str()), "the refusal of a {BYTES}-byte pattern does not say `{part}`: {text}");
    }
}

/// A start that went past the names to connect, which a port refusing connections fails.
async fn served<const BYTES: usize, const EVERY: bool>() {
    let port = TcpListener::bind("127.0.0.1:0").expect("a loopback port").local_addr().expect("the port's address").port();
    let error = started::<BYTES, EVERY>(port).await;
    assert!(!ulo::is_permanent(&error), "a {BYTES}-byte pattern was refused rather than connected for: {error}");
    let StartupError::Bind { source, .. } = error else { panic!("expected a `Bind` failure, got: {error}") };
    let cause = source.into_inner();
    let mut link: Option<&(dyn Error + 'static)> = Some(&*cause);
    let connected = std::iter::from_fn(|| {
        let current = link?;
        link = current.source();
        Some(current)
    })
    .filter_map(|error| error.downcast_ref::<std::io::Error>())
    .any(|io| io.kind() == ErrorKind::ConnectionRefused);
    assert!(connected, "the start on a {BYTES}-byte pattern did not reach the connect: {cause}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_241_byte_pattern_is_served_and_a_242_byte_one_refused_before_the_connect() {
    served::<241, false>().await;
    refused::<242, false>(&[format!("pattern `{}` is 242 bytes", pattern(242)), "256 bytes".into(), "AMQP's 255-byte limit".into()]).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_every_instance_event_on_208_bytes_is_served_and_on_209_refused_before_the_connect() {
    served::<208, true>().await;
    refused::<209, true>(&[format!("pattern `{}` is 209 bytes and its event is declared `every_instance`", pattern(209)), "256 bytes".into(), "AMQP's 255-byte limit".into()])
        .await;
}
