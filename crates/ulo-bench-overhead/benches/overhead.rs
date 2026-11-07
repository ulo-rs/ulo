//! Framework overhead: `GET /` answering a fixed body, served by bare hyper and by ulo's hyper
//! backend, each driven over loopback by the same load loop.
//!
//! Both servers run HTTP/1.1 on a multi-thread tokio runtime of the same size, behind the same
//! `http1::Builder` settings, and answer the same status, headers and body; the harness checks
//! that before it measures. Each round starts a fresh server per target and alternates which
//! target goes first. Every figure reported is the median over the rounds; the `ulo / bare` row is
//! the median of each round's ratio, so a machine that slows during the run moves both targets of
//! a round together.
//!
//! Settings come from the environment: `ULO_BENCH_WORKERS` (server and client worker threads, 4),
//! `ULO_BENCH_CONNECTIONS` (keep-alive connections, 64), `ULO_BENCH_SECONDS` (measured seconds per
//! round, 5) and `ULO_BENCH_ROUNDS` (rounds, 5). Warm-up is one second per round.

use std::convert::Infallible;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

use bytes::Bytes;
use http_body_util::{BodyExt, Empty, Full};
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper::{Request, Response, header};
use hyper_util::rt::{TokioIo, TokioTimer};
use tokio::net::{TcpListener, TcpStream};
use tokio::runtime::{Builder, Runtime};
use ulo::{App, Signal, controller, handlers, module};
use ulo_http_hyper::Server;
use ulo_runtime_tokio::Tokio;

const BODY: &str = "Hello, ulo!";
const CONTENT_TYPE: &str = "text/plain; charset=utf-8";
/// The header-read timeout `ulo_http::Server` applies at its default, which bare hyper is given too.
const HEADER_TIMEOUT: Duration = Duration::from_secs(30);
const WARMUP: Duration = Duration::from_secs(1);

#[controller]
struct Hello;

#[handlers]
impl Hello {
    #[ulo_http::get("/")]
    fn hello(&self) -> &'static str {
        BODY
    }
}

#[module(controllers = [Hello])]
struct AppModule;

#[derive(Clone, Copy)]
struct Settings {
    workers: usize,
    connections: usize,
    seconds: u64,
    rounds: usize,
}

fn setting(name: &str, default: usize) -> usize {
    match std::env::var(name) {
        Ok(value) => value.parse().unwrap_or_else(|_| panic!("{name}={value} is not a number")),
        Err(_) => default,
    }
}

/// A server on its own runtime: dropping it drops the listener and every connection task.
struct Running {
    addr: SocketAddr,
    _runtime: Runtime,
}

fn server_runtime(workers: usize) -> Runtime {
    Builder::new_multi_thread().worker_threads(workers).enable_all().build().expect("the server runtime builds")
}

fn bare_hyper(workers: usize) -> Running {
    let runtime = server_runtime(workers);
    let listener = runtime.block_on(TcpListener::bind("127.0.0.1:0")).expect("the bare server binds");
    let addr = listener.local_addr().expect("the bare server has an address");
    runtime.spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else { continue };
            tokio::spawn(async move {
                let service = service_fn(|_request: Request<Incoming>| async {
                    let mut response = Response::new(Full::new(Bytes::from_static(BODY.as_bytes())));
                    response.headers_mut().insert(header::CONTENT_TYPE, header::HeaderValue::from_static(CONTENT_TYPE));
                    Ok::<_, Infallible>(response)
                });
                let mut http1 = hyper::server::conn::http1::Builder::new();
                http1.timer(TokioTimer::new()).header_read_timeout(HEADER_TIMEOUT);
                let _ = http1.serve_connection(TokioIo::new(stream), service).with_upgrades().await;
            });
        }
    });
    Running { addr, _runtime: runtime }
}

fn ulo_server(workers: usize) -> Running {
    let runtime = server_runtime(workers);
    let addr = runtime.block_on(async {
        let app = App::builder(AppModule)
            .runtime(Tokio::current())
            .wire()
            .expect("the app wires")
            .connect()
            .await
            .expect("the app connects")
            .bind(Server::new("127.0.0.1:0"))
            .listen()
            .await
            .expect("the app listens");
        let addr = app.addresses().first().expect("the app has a bound address").addr;
        tokio::spawn(async move {
            let _ = app.serve(std::future::pending::<Signal>()).await;
        });
        addr
    });
    Running { addr, _runtime: runtime }
}

/// One keep-alive connection's request loop: every response is checked, and a latency recorded
/// once the warm-up is over, until the measured window ends.
async fn connection(addr: SocketAddr, warmed_at: Instant, ends_at: Instant) -> Vec<u64> {
    let stream = TcpStream::connect(addr).await.expect("the client connects");
    let (mut sender, driver) = hyper::client::conn::http1::handshake::<_, Empty<Bytes>>(TokioIo::new(stream)).await.expect("the client handshakes");
    tokio::spawn(driver);
    let mut latencies = Vec::new();
    loop {
        let started = Instant::now();
        if started >= ends_at {
            return latencies;
        }
        let request = Request::get("/").header(header::HOST, "bench").body(Empty::<Bytes>::new()).expect("the request builds");
        let response = sender.send_request(request).await.expect("the request is answered");
        assert_eq!(response.status(), 200, "the server answered a status other than 200");
        let body = response.into_body().collect().await.expect("the body arrives").to_bytes();
        assert_eq!(body, BODY.as_bytes(), "the server answered another body");
        if started >= warmed_at {
            latencies.push(started.elapsed().as_nanos() as u64);
        }
    }
}

struct Measured {
    requests_per_second: f64,
    p50_us: f64,
    p99_us: f64,
}

fn drive(client: &Runtime, addr: SocketAddr, settings: Settings) -> Measured {
    let started = Instant::now();
    let warmed_at = started + WARMUP;
    let ends_at = warmed_at + Duration::from_secs(settings.seconds);
    let mut latencies: Vec<u64> = client.block_on(async {
        let tasks: Vec<_> = (0..settings.connections).map(|_| tokio::spawn(connection(addr, warmed_at, ends_at))).collect();
        let mut all = Vec::new();
        for task in tasks {
            all.extend(task.await.expect("a connection task finishes"));
        }
        all
    });
    latencies.sort_unstable();
    let percentile = |p: f64| latencies[((latencies.len() as f64 * p).ceil() as usize).clamp(1, latencies.len()) - 1] as f64 / 1_000.0;
    Measured {
        requests_per_second: latencies.len() as f64 / settings.seconds as f64,
        p50_us: percentile(0.50),
        p99_us: percentile(0.99),
    }
}

/// What a client sees of `GET /`: the status, the headers but for `date`, and the body.
fn head(client: &Runtime, addr: SocketAddr) -> (u16, Vec<(String, String)>, Bytes) {
    client.block_on(async {
        let stream = TcpStream::connect(addr).await.expect("the client connects");
        let (mut sender, driver) = hyper::client::conn::http1::handshake::<_, Empty<Bytes>>(TokioIo::new(stream)).await.expect("the client handshakes");
        tokio::spawn(driver);
        let request = Request::get("/").header(header::HOST, "bench").body(Empty::<Bytes>::new()).expect("the request builds");
        let response = sender.send_request(request).await.expect("the request is answered");
        let status = response.status().as_u16();
        let mut headers: Vec<_> = response
            .headers()
            .iter()
            .filter(|(name, _)| *name != header::DATE)
            .map(|(name, value)| (name.to_string(), value.to_str().unwrap_or("<binary>").to_owned()))
            .collect();
        headers.sort();
        let body = response.into_body().collect().await.expect("the body arrives").to_bytes();
        (status, headers, body)
    })
}

fn median(mut values: Vec<f64>) -> f64 {
    values.sort_by(f64::total_cmp);
    values[values.len() / 2]
}

fn main() {
    let settings = Settings {
        workers: setting("ULO_BENCH_WORKERS", 4),
        connections: setting("ULO_BENCH_CONNECTIONS", 64),
        seconds: setting("ULO_BENCH_SECONDS", 5) as u64,
        rounds: setting("ULO_BENCH_ROUNDS", 5),
    };
    let client = Builder::new_multi_thread().worker_threads(settings.workers).enable_all().build().expect("the client runtime builds");

    // The two servers must answer alike, or the difference measured is a different response.
    let (bare, ulo) = (bare_hyper(settings.workers), ulo_server(settings.workers));
    let (bare_head, ulo_head) = (head(&client, bare.addr), head(&client, ulo.addr));
    println!("bare hyper answers {bare_head:?}");
    println!("ulo answers        {ulo_head:?}");
    assert_eq!(bare_head, ulo_head, "the two servers answer differently");
    drop((bare, ulo));

    println!(
        "{} connections, {} worker threads each for server and client, {}s measured after {}s warm-up, {} rounds",
        settings.connections,
        settings.workers,
        settings.seconds,
        WARMUP.as_secs(),
        settings.rounds
    );
    let mut bare_rounds = Vec::new();
    let mut ulo_rounds = Vec::new();
    for round in 0..settings.rounds {
        // Alternate the first target, so neither always runs on a cold or a warm machine.
        let order: [bool; 2] = if round % 2 == 0 { [true, false] } else { [false, true] };
        for is_bare in order {
            let server = if is_bare { bare_hyper(settings.workers) } else { ulo_server(settings.workers) };
            let measured = drive(&client, server.addr, settings);
            drop(server);
            println!(
                "round {round} {:<10} {:>9.0} req/s  p50 {:>7.1} us  p99 {:>7.1} us",
                if is_bare { "bare hyper" } else { "ulo" },
                measured.requests_per_second,
                measured.p50_us,
                measured.p99_us
            );
            if is_bare { &mut bare_rounds } else { &mut ulo_rounds }.push(measured);
        }
    }

    let summary = |rounds: &[Measured]| {
        (
            median(rounds.iter().map(|m| m.requests_per_second).collect()),
            median(rounds.iter().map(|m| m.p50_us).collect()),
            median(rounds.iter().map(|m| m.p99_us).collect()),
        )
    };
    let (bare, ulo) = (summary(&bare_rounds), summary(&ulo_rounds));
    let paired = |column: fn(&Measured) -> f64| median(bare_rounds.iter().zip(&ulo_rounds).map(|(b, u)| column(u) / column(b)).collect());
    let ratio = (paired(|m| m.requests_per_second), paired(|m| m.p50_us), paired(|m| m.p99_us));
    println!("\nmedian over {} rounds", settings.rounds);
    println!("{:<12} {:>12} {:>12} {:>12}", "", "req/s", "p50 us", "p99 us");
    println!("{:<12} {:>12.0} {:>12.1} {:>12.1}", "bare hyper", bare.0, bare.1, bare.2);
    println!("{:<12} {:>12.0} {:>12.1} {:>12.1}", "ulo", ulo.0, ulo.1, ulo.2);
    println!("{:<12} {:>12.3} {:>12.3} {:>12.3}", "ulo / bare", ratio.0, ratio.1, ratio.2);
}
