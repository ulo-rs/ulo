//! `GrpcClientModule`: the generated client bound under its own type, built through the
//! `GrpcClient` impl `ulo-build` wrote, keyed so two endpoints of one service coexist, its URI
//! checked when the app wires and its channel connected lazily; and `outgoing`, which forwards an
//! execution's deadline as `grpc-timeout` and refuses one already passed, its value measured before
//! a lazily connected channel makes its connection.

mod support;

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use hyper_util::client::legacy::connect::HttpConnector;
use tonic::codegen::Service;
use tonic::transport::{Channel, Endpoint, Uri};
use tonic::{Code, Status};
use ulo::{App, Dep, ExecOptions, ExecutionRef, Module, ModuleDef, ModuleIdentity, Signal, injectable, routes};
use ulo_codegen_tests::probe::clock_client::ClockClient;
use ulo_codegen_tests::probe::known_client::KnownClient;
use ulo_codegen_tests::probe::{self, Tick, Ticks};
use ulo_grpc::{GrpcClientModule, GrpcEndpoint, Message};

use support::{Running, within};

/// The name a server answers `Upper` with, so a caller can tell two servers apart.
struct ServerName(&'static str);

#[injectable]
struct Known {
    name: Dep<ServerName>,
}

#[routes]
impl Known {
    #[ulo_grpc::method(probe::known::Upper)]
    fn upper(&self, text: Message<String>) -> String {
        format!("{}:{}", self.name.0, text.0.to_uppercase())
    }
}

/// How many calls reached `Clock::quick`, and the deadline each was given.
#[derive(Clone, Default)]
struct Ran {
    count: Arc<AtomicUsize>,
    deadlines: Arc<Mutex<Vec<Option<Instant>>>>,
}

impl Ran {
    fn count(&self) -> usize {
        self.count.load(Ordering::SeqCst)
    }

    fn deadlines(&self) -> Vec<Option<Instant>> {
        self.deadlines.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

#[injectable]
struct Clock {
    ran: Dep<Ran>,
}

#[routes]
impl Clock {
    /// The time left before the call's deadline, in whole milliseconds rounded up; `0` for none.
    #[ulo_grpc::method(probe::clock::Quick)]
    fn quick(&self, _req: Message<Ticks>, exec: ulo::ExecutionRef) -> Tick {
        self.ran.count.fetch_add(1, Ordering::SeqCst);
        self.ran.deadlines.lock().unwrap_or_else(PoisonError::into_inner).push(exec.deadline());
        let left = exec.deadline().map_or(0, |deadline| {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now().into_std());
            u32::try_from(left.as_millis()).unwrap_or(u32::MAX).saturating_add(1)
        });
        Tick { n: left }
    }
}

struct ServerRoot(&'static str, Ran);

impl Module for ServerRoot {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.value(ServerName(self.0));
        m.value(self.1.clone());
        m.controller::<Known>();
        m.controller::<Clock>();
    }
}

async fn server(name: &'static str) -> Running {
    counted(name).await.0
}

/// A server, and the count of calls that reached its `Clock` handlers.
async fn counted(name: &'static str) -> (Running, Ran) {
    let ran = Ran::default();
    (Running::start(ServerRoot(name, ran.clone()), ulo_grpc::Server::new("127.0.0.1:0")).await, ran)
}

struct Primary;
struct Secondary;

type KnownGrpc = KnownClient<Channel>;

/// Reaches both servers, each through its own keyed client.
#[injectable]
struct Caller {
    primary: Dep<KnownGrpc, Primary>,
    secondary: Dep<KnownGrpc, Secondary>,
}

/// The client app's root: the given client modules, and `Caller` when both keyed ones are among
/// them.
struct ClientRoot {
    imports: Vec<Import>,
}

#[derive(Clone)]
enum Import {
    Plain(String),
    Keyed(String, String),
}

impl Module for ClientRoot {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        for import in &self.imports {
            match import.clone() {
                Import::Plain(uri) => m.import(GrpcClientModule::<KnownGrpc>::for_root(GrpcEndpoint::new(uri))),
                Import::Keyed(primary, secondary) => {
                    m.import(GrpcClientModule::<KnownGrpc>::for_root(GrpcEndpoint::new(primary)).keyed::<Primary>());
                    m.import(GrpcClientModule::<KnownGrpc>::for_root(GrpcEndpoint::new(secondary)).keyed::<Secondary>());
                    m.provide::<Caller>();
                }
            }
        }
    }
}

/// The client app, connected; it binds no server.
async fn client_app(imports: Vec<Import>) -> App<ulo::Connected> {
    App::builder(ClientRoot { imports })
        .timer(ulo_runtime_tokio::Timer::current())
        .wire()
        .unwrap_or_else(|error| panic!("the client app did not wire: {error}"))
        .connect()
        .await
        .unwrap_or_else(|error| panic!("the client app did not connect: {error}"))
}

async fn upper(client: &KnownGrpc, text: &str) -> Result<String, tonic::Status> {
    let mut client = client.clone();
    within("the Upper call", client.upper(text.to_owned())).await.map(tonic::Response::into_inner)
}

#[tokio::test]
async fn the_bound_client_calls_its_endpoint() {
    let server = server("a").await;
    let app = client_app(vec![Import::Plain(server.origin())]).await;
    let client = app.get::<KnownGrpc>().await.unwrap_or_else(|error| panic!("the client did not resolve: {error}"));
    assert_eq!(upper(&client, "ada").await.map_err(|status| status.code()), Ok("a:ADA".to_owned()));
    let _ = app.close(Signal::new("test")).await;
    server.stop().await;
}

#[tokio::test]
async fn keyed_clients_of_one_service_each_reach_their_own_endpoint() {
    let first = server("first").await;
    let second = server("second").await;
    let app = client_app(vec![Import::Keyed(first.origin(), second.origin())]).await;
    let caller = app.get::<Caller>().await.unwrap_or_else(|error| panic!("Caller did not resolve: {error}"));
    assert_eq!(upper(&caller.primary, "x").await.map_err(|status| status.code()), Ok("first:X".to_owned()));
    assert_eq!(upper(&caller.secondary, "x").await.map_err(|status| status.code()), Ok("second:X".to_owned()));
    let _ = app.close(Signal::new("test")).await;
    first.stop().await;
    second.stop().await;
}

#[tokio::test]
async fn connect_does_no_network_io_and_the_first_call_finds_the_endpoint_down() {
    // Port 0 names no endpoint: nothing can listen on it, binding it choosing another port, so the
    // TCP connect fails on every OS (refused on Linux, `EADDRNOTAVAIL` on macOS) and no process can
    // take the port first.
    let app = client_app(vec![Import::Plain("http://127.0.0.1:0".to_owned())]).await;
    let client = app.get::<KnownGrpc>().await.unwrap_or_else(|error| panic!("the client did not resolve: {error}"));
    assert_eq!(upper(&client, "ada").await.map_err(|status| status.code()), Err(Code::Unavailable));
    let _ = app.close(Signal::new("test")).await;
}

#[tokio::test]
async fn a_uri_that_does_not_parse_fails_wiring() {
    let wired = App::builder(ClientRoot { imports: vec![Import::Plain("not a uri".to_owned())] }).timer(ulo_runtime_tokio::Timer::current()).wire();
    let error = match wired {
        Err(error) => error.to_string(),
        Ok(_) => panic!("an endpoint URI that does not parse wired"),
    };
    assert!(error.contains("`not a uri` is not a gRPC endpoint URI"), "the wiring error does not name the URI: {error}");
}

#[tokio::test]
async fn outgoing_forwards_the_execution_s_remaining_deadline() {
    let server = server("a").await;
    let app = client_app(Vec::new()).await;
    let (http2, origin) = server.plain_http2();
    let mut clock = ClockClient::with_origin(http2, origin);

    let deadline = tokio::time::Instant::now().into_std() + Duration::from_secs(2);
    let left = app
        .execute(ExecOptions::new().deadline(deadline), async |exec| {
            let request = forward(&exec.handle()).unwrap_or_else(|status| panic!("a deadline 2 s off was refused: {status:?}"));
            clock.quick(request).await.map(|reply| reply.into_inner().n)
        })
        .await
        .unwrap_or_else(|error| panic!("the execution was refused: {error}"))
        .unwrap_or_else(|status| panic!("Quick failed: {status:?}"));
    assert!((1..=2000).contains(&left), "{left} ms left of a 2 s deadline");
    let _ = app.close(Signal::new("test")).await;
    server.stop().await;
}

/// A `Quick` request forwarding `exec`'s deadline, as a handler calling out writes it.
fn forward(exec: &ExecutionRef) -> Result<tonic::Request<Ticks>, Status> {
    ulo_grpc::outgoing(exec, tonic::Request::new(Ticks::default()))
}

/// A call forwarding the deadline of an execution whose deadline has passed fails before it is
/// made, DEADLINE_EXCEEDED, and its server runs nothing: through a plain HTTP/2 client and through
/// a tonic `Channel`, which enforces `grpc-timeout` itself, `Quick`, which answers without
/// awaiting, has run once afterwards, for a call made outside the execution.
#[tokio::test]
async fn outgoing_refuses_a_passed_deadline_so_nothing_reaches_the_server() {
    let (server, ran) = counted("a").await;
    let app = client_app(Vec::new()).await;
    let (http2, origin) = server.plain_http2();
    let mut plain = ClockClient::with_origin(http2, origin);
    let mut channel = ClockClient::new(server.channel().await);
    let passed = tokio::time::Instant::now().into_std();
    let outcomes = app
        .execute(ExecOptions::new().deadline(passed), async |exec| {
            let exec = exec.handle();
            let through_plain = match forward(&exec) {
                Ok(request) => within("the plain client's call", plain.quick(request)).await.map(|reply| reply.into_inner().n),
                Err(status) => Err(status),
            };
            let through_channel = match forward(&exec) {
                Ok(request) => within("the channel's call", channel.quick(request)).await.map(|reply| reply.into_inner().n),
                Err(status) => Err(status),
            };
            [("a plain HTTP/2 client", through_plain), ("a tonic `Channel`", through_channel)]
        })
        .await
        .unwrap_or_else(|error| panic!("the execution was refused: {error}"));
    let outside = within("the call outside the execution", plain.quick(Ticks::default())).await;
    assert_eq!(outside.map(|reply| reply.into_inner().n).map_err(|status| status.code()), Ok(0), "a call without a deadline");
    assert_eq!(ran.count(), 1, "the calls that reached `Quick`: only the one outside the execution; the others answered {outcomes:?}");
    for (via, outcome) in outcomes {
        assert_eq!(outcome.map_err(|status| status.code()), Err(Code::DeadlineExceeded), "a call past its deadline through {via}");
    }
    let _ = app.close(Signal::new("test")).await;
    server.stop().await;
}

/// An execution whose deadline is further off than the longest `grpc-timeout`, `99999999H`, about
/// 11,415 years, forwards that longest value, where tonic alone would panic on it.
#[tokio::test]
async fn outgoing_forwards_a_deadline_beyond_the_grammar_as_its_longest_value() {
    let app = client_app(Vec::new()).await;
    let far = tokio::time::Instant::now().into_std() + Duration::from_secs(400_000_000_000);
    let written = app
        .execute(ExecOptions::new().deadline(far), async |exec| {
            let request = forward(&exec.handle()).unwrap_or_else(|status| panic!("a deadline 400 billion seconds off was refused: {status:?}"));
            request.metadata().get("grpc-timeout").map(|value| value.to_str().map(str::to_owned))
        })
        .await
        .unwrap_or_else(|error| panic!("the execution was refused: {error}"));
    assert_eq!(written.map(|value| value.map_err(|error| error.to_string())), Some(Ok("99999999H".to_owned())));
    let _ = app.close(Signal::new("test")).await;
}

/// A connector holding each connect for `held` before making it, recording when each began and when
/// it returned, on the test's clock.
#[derive(Clone)]
struct SlowConnector {
    inner: HttpConnector,
    held: Duration,
    connects: Arc<Mutex<Vec<(Instant, Instant)>>>,
}

impl Service<Uri> for SlowConnector {
    type Response = <HttpConnector as Service<Uri>>::Response;
    type Error = <HttpConnector as Service<Uri>>::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, uri: Uri) -> Self::Future {
        let mut inner = self.inner.clone();
        let (held, connects) = (self.held, Arc::clone(&self.connects));
        Box::pin(async move {
            let began = tokio::time::Instant::now().into_std();
            tokio::time::sleep(held).await;
            let io = inner.call(uri).await?;
            connects.lock().unwrap_or_else(PoisonError::into_inner).push((began, tokio::time::Instant::now().into_std()));
            Ok(io)
        })
    }
}

/// A `grpc-timeout` value as a duration: an integer and a unit, `H`, `M`, `S`, `m`, `u` or `n`.
fn grpc_timeout(value: &str) -> Duration {
    let (digits, unit) = value.split_at(value.len() - 1);
    let n: u64 = digits.parse().unwrap_or_else(|error| panic!("`grpc-timeout: {value}` does not parse: {error}"));
    match unit {
        "H" => Duration::from_secs(n * 3600),
        "M" => Duration::from_secs(n * 60),
        "S" => Duration::from_secs(n),
        "m" => Duration::from_millis(n),
        "u" => Duration::from_micros(n),
        "n" => Duration::from_nanos(n),
        other => panic!("`grpc-timeout: {value}` has no unit `{other}`"),
    }
}

/// `a - b` in milliseconds, negative when `a` is the earlier.
fn signed_millis(a: Instant, b: Instant) -> f64 {
    match a.checked_duration_since(b) {
        Some(later) => later.as_secs_f64() * 1e3,
        None => -(b.duration_since(a).as_secs_f64() * 1e3),
    }
}

/// F410's standing limit on gRPC, pinned so it grows no larger. `outgoing` writes `grpc-timeout`
/// before the request reaches tonic's `Channel`, whose connection the request itself drives from
/// inside the channel, so on a new connection the server is given the connect's length beyond the
/// caller's deadline. Three rounds, each the first call over a channel of its own whose connector
/// holds the connect a quarter second, from an execution with two seconds left. What the server
/// is given beyond the caller's deadline, less the connect, measured by the connector from its
/// start to its end, and less the request's transit, from the connect's end to the server's
/// reading of `grpc-timeout`, is the time from `outgoing`'s reading of the clock to the connect's
/// start. The test reads the clock on either side of `outgoing`, so that gap lies between what the
/// two readings give, less one unit of the `grpc-timeout` written, which tonic truncates to. A
/// stamp written later than `outgoing` returns, after the connect among them, fails the lower
/// bound; one written earlier than it is called, or a `grpc-timeout` longer than the time left,
/// the upper.
#[tokio::test]
async fn a_new_connection_gives_the_server_the_connect_beyond_the_caller_s_deadline_and_no_more() {
    let (server, ran) = counted("a").await;
    let app = client_app(Vec::new()).await;
    let held = Duration::from_millis(250);
    let mut report = Vec::new();
    let (mut late, mut early) = (false, false);
    for round in 0..3 {
        let connects = Arc::new(Mutex::new(Vec::new()));
        let connector = SlowConnector { inner: HttpConnector::new(), held, connects: Arc::clone(&connects) };
        let endpoint = Endpoint::from_shared(server.origin()).unwrap_or_else(|error| panic!("bad endpoint: {error}"));
        let mut clock = ClockClient::new(endpoint.connect_with_connector_lazy(connector));
        let deadline = tokio::time::Instant::now().into_std() + Duration::from_secs(2);
        let (before, after, written, outcome) = app
            .execute(ExecOptions::new().deadline(deadline), async |exec| {
                let before = Instant::now();
                let request = forward(&exec.handle()).unwrap_or_else(|status| panic!("a deadline 2 s off was refused: {status:?}"));
                let after = Instant::now();
                let written = request.metadata().get("grpc-timeout").and_then(|value| value.to_str().ok()).map(str::to_owned);
                (before, after, written, within("the call over a new connection", clock.quick(request)).await)
            })
            .await
            .unwrap_or_else(|error| panic!("the execution was refused: {error}"));
        if let Err(status) = outcome {
            panic!("round {round}: Quick over a new connection failed: {status:?}");
        }
        let written = written.unwrap_or_else(|| panic!("round {round}: `outgoing` wrote no `grpc-timeout`"));
        let server_deadline = ran.deadlines().last().copied().flatten().unwrap_or_else(|| panic!("round {round}: the server gave the call no deadline"));
        let (began, connected) = match connects.lock().unwrap_or_else(PoisonError::into_inner).as_slice() {
            [connect] => *connect,
            other => panic!("round {round}: expected one connect, got {other:?}"),
        };
        let read = server_deadline - grpc_timeout(&written);
        let (connect, transit) = (connected - began, read.saturating_duration_since(connected));
        // The server's deadline beyond the caller's, less the connect and the transit, in
        // milliseconds: the gap from `outgoing`'s reading of the clock to the connect's start.
        let gap = signed_millis(server_deadline, deadline) - (connect + transit).as_secs_f64() * 1e3;
        let unit = grpc_timeout(&format!("1{}", &written[written.len() - 1..])).as_secs_f64() * 1e3;
        late |= gap > signed_millis(began, before);
        early |= gap < signed_millis(began, after) - unit;
        report.push(format!(
            "round {round}: `grpc-timeout: {written}`, the server's deadline {:+.3} ms from the caller's, the connect {:.3} ms, the request {:.3} ms in transit, a gap of {gap:.6} ms against {:.6} to {:.6} ms",
            signed_millis(server_deadline, deadline),
            connect.as_secs_f64() * 1e3,
            transit.as_secs_f64() * 1e3,
            signed_millis(began, after) - unit,
            signed_millis(began, before)
        ));
    }
    assert!(!late, "a server was given more than the connect, the transit and the time from `outgoing` to the connect:\n{}", report.join("\n"));
    assert!(!early, "a server was given less than the connect, the transit and the time from `outgoing`'s return to the connect:\n{}", report.join("\n"));
    let _ = app.close(Signal::new("test")).await;
    server.stop().await;
}
