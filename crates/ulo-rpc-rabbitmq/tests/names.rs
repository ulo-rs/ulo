//! Q29-9, Q30-7: a pattern whose exchange or queue name is over AMQP's 255 bytes fails the start
//! in `prepare`, as a permanent `Configure` failure naming the pattern, its length and the limit,
//! reported with the other servers' configuration failures before any server binds; a pattern at
//! the limit goes on to connect. Patterns of 241 bytes, and of 208 for an event declared
//! `every_instance`, are the longest served. No broker is needed: the refused start leaves a
//! listener the test holds unconnected to, and the served one reaches a port that refuses
//! connections.

use std::error::Error;
use std::io::ErrorKind;
use std::net::TcpListener;

use ulo::{App, BoxError, Construct, ConstructError, Controller, Dependencies, HandlerSpec, Handlers, Module, ModuleDef, ModuleIdentity};
use ulo::{Mount, Resolver, Signal, StartupError};
use ulo_rpc::{Reply, Rpc, RpcCx, RpcHandler, Server};
use ulo_rpc_conformance::relay::refusing_addr;
use ulo_rpc_rabbitmq::RabbitMq;
use ulo_runtime_tokio::Tokio;
use ulo_transport::Count;

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

/// A server on a link to `port`.
fn server(port: u16) -> Server<RabbitMq> {
    Server::new(RabbitMq::url(format!("amqp://guest:guest@127.0.0.1:{port}/%2f")))
}

/// A loopback listener nothing accepts on, which a test reads for a connection the start made.
fn unaccepted() -> TcpListener {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
    listener.set_nonblocking(true).expect("the listener goes non-blocking");
    listener
}

fn port(listener: &TcpListener) -> u16 {
    listener.local_addr().expect("the listener's address").port()
}

fn connected(listener: &TcpListener) -> bool {
    !matches!(listener.accept(), Err(error) if error.kind() == ErrorKind::WouldBlock)
}

/// The start of an app whose one handler is `Root`'s, binding `servers` in order.
async fn started_with<const BYTES: usize, const EVERY: bool>(servers: Vec<Server<RabbitMq>>) -> StartupError {
    let mut app = App::builder(Root::<BYTES, EVERY>).runtime(Tokio::current()).wire().expect("the app wires").connect().await.expect("the app connects");
    for server in servers {
        app = app.bind(server);
    }
    match app.listen().await {
        Ok(app) => {
            let _ = app.handle().close(Signal::new("listened")).await;
            panic!("a server listened on a {BYTES}-byte pattern with no broker");
        }
        Err(error) => error,
    }
}

/// The start of a server whose one handler is `Root`'s, on a link to `port`.
async fn started<const BYTES: usize, const EVERY: bool>(port: u16) -> StartupError {
    started_with::<BYTES, EVERY>(vec![server(port)]).await
}

/// A start refused for a pattern's names: before any connection to the listener the link was
/// pointed at, as a permanent `Configure` whose one entry's text holds `parts`.
async fn refused<const BYTES: usize, const EVERY: bool>(parts: &[String]) {
    let listener = unaccepted();
    let error = started::<BYTES, EVERY>(port(&listener)).await;
    assert!(!connected(&listener), "the link connected before refusing a {BYTES}-byte pattern");
    assert!(ulo::is_permanent(&error), "a pattern AMQP cannot name is refused as retryable: {error}");
    let StartupError::Configure(errors) = error else { panic!("expected a `Configure` failure, got: {error}") };
    let [entry] = errors.iter().collect::<Vec<_>>()[..] else { panic!("expected one server's failure, got {}: {errors:?}", errors.len()) };
    let text = entry.source.to_string();
    for part in parts {
        assert!(text.contains(part.as_str()), "the refusal of a {BYTES}-byte pattern does not say `{part}`: {text}");
    }
}

/// A start that went past the names to connect, which a port refusing connections fails.
async fn served<const BYTES: usize, const EVERY: bool>() {
    let error = started::<BYTES, EVERY>(refusing_addr().port()).await;
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

/// Q30-7: the refusal is collected with the configuration failures of every server before any
/// binds: two servers on a 242-byte pattern, the first also refusing `max_inflight(Count::Max(0))`,
/// each report their own, and neither link connects. Planted break: the check left in `listen`,
/// where the first server's refusal is a `Bind` and the second is never prepared past it.
#[tokio::test(flavor = "multi_thread")]
async fn the_refusal_is_reported_with_every_server_s_configuration_failures_before_any_bind() {
    let (first, second) = (unaccepted(), unaccepted());
    let error = started_with::<242, false>(vec![server(port(&first)).max_inflight(Count::Max(0)), server(port(&second))]).await;
    assert!(!connected(&first) && !connected(&second), "a link connected before the configuration was refused");
    let StartupError::Configure(errors) = error else { panic!("expected a `Configure` failure, got: {error}") };
    let texts: Vec<String> = errors.iter().map(|entry| entry.source.to_string()).collect();
    assert_eq!(texts.len(), 2, "one entry per server: {texts:?}");
    for text in &texts {
        assert!(text.contains("is 242 bytes"), "a server's entry does not name the pattern: {text}");
    }
    assert!(texts[0].contains("max_inflight"), "the first server's entry does not hold its zero `max_inflight`: {}", texts[0]);
    assert!(!texts[1].contains("max_inflight"), "the second server's entry holds the first's `max_inflight`: {}", texts[1]);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_every_instance_event_on_208_bytes_is_served_and_on_209_refused_before_the_connect() {
    served::<208, true>().await;
    refused::<209, true>(&[format!("pattern `{}` is 209 bytes and its event is declared `every_instance`", pattern(209)), "256 bytes".into(), "AMQP's 255-byte limit".into()])
        .await;
}
