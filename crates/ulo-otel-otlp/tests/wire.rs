//! What reaches a local listener when `OtlpExporter` exports one span.
//!
//! The same scenarios run under `--features reqwest-blocking`, which swaps the HTTP client.
//!
//! The process environment is set only on a child process (the test binary re-run on `child`), so
//! no test mutates its own.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::task::{Context, Waker};
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime};

use opentelemetry::trace::{SpanContext, SpanId, SpanKind, Status, TraceFlags, TraceId, TraceState};
use opentelemetry::InstrumentationScope;
use opentelemetry_sdk::trace::{SpanData, SpanExporter};
use ulo::Secret;
use ulo_otel::{Exporter, OtelOptions};
use ulo_otel_otlp::OtlpExporter;

const CHILD: &str = "ULO_OTLP_CHILD";

/// Accepts one connection and answers it; `head` stops waiting and answers `None` when nothing
/// connected by then.
struct Capture {
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    handle: JoinHandle<Option<String>>,
}

fn listen(reply: &'static str) -> Capture {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let addr = listener.local_addr().unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let stopped = stop.clone();
    let handle = std::thread::spawn(move || {
        let mut stream = loop {
            // `stopped` is read before the accept, so a connection made before the stop is seen.
            let last = stopped.load(Ordering::SeqCst);
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(_) if !last => std::thread::sleep(Duration::from_millis(10)),
                Err(_) => return None,
            }
        };
        stream.set_nonblocking(false).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        let mut head = Vec::new();
        let mut buf = [0u8; 4096];
        // A TLS client waits for the server's hello, so its first record is the whole head.
        while !head.windows(4).any(|w| w == b"\r\n\r\n") && head.first() != Some(&TLS_HANDSHAKE) {
            match stream.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => head.extend_from_slice(&buf[..n]),
            }
        }
        let _ = stream.write_all(reply.as_bytes());
        Some(String::from_utf8_lossy(&head).into_owned())
    });
    Capture { addr, stop, handle }
}

const OK: &str = "HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n";
/// The content type of a TLS handshake record, the first byte a TLS client writes.
const TLS_HANDSHAKE: u8 = 0x16;
const REFUSE: &str = "HTTP/1.1 403 Forbidden\r\ncontent-length: 0\r\n\r\n";

impl Capture {
    fn url(&self) -> String {
        format!("http://{}", self.addr)
    }
    fn head(self) -> Option<String> {
        self.stop.store(true, Ordering::SeqCst);
        self.handle.join().unwrap()
    }
}

/// Every value the request head gives `name`, in order.
fn header_values(head: &str, name: &str) -> Vec<String> {
    head.lines()
        .skip(1)
        .filter_map(|l| l.split_once(':'))
        .filter(|(k, _)| k.trim().eq_ignore_ascii_case(name))
        .map(|(_, v)| v.trim().to_owned())
        .collect()
}

fn run_child(envs: &[(&str, &str)]) {
    let mut cmd = Command::new(std::env::current_exe().unwrap());
    cmd.args(["--exact", "child", "--nocapture", "--test-threads=1"]);
    for var in [
        "ALL_PROXY", "all_proxy", "HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy", "NO_PROXY",
        "no_proxy", "OTEL_EXPORTER_OTLP_HEADERS", "OTEL_EXPORTER_OTLP_TRACES_HEADERS",
        "OTEL_EXPORTER_OTLP_ENDPOINT", "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT",
    ] {
        cmd.env_remove(var);
    }
    for (k, v) in envs {
        cmd.env(k, v);
    }
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "child failed: {}",
        String::from_utf8_lossy(&out.stdout)
    );
}

fn span() -> SpanData {
    SpanData {
        span_context: SpanContext::new(
            TraceId::from_bytes([1; 16]),
            SpanId::from_bytes([2; 8]),
            TraceFlags::SAMPLED,
            false,
            TraceState::default(),
        ),
        parent_span_id: SpanId::INVALID,
        parent_span_is_remote: false,
        span_kind: SpanKind::Internal,
        name: "wire".into(),
        start_time: SystemTime::now(),
        end_time: SystemTime::now(),
        attributes: Vec::new(),
        dropped_attributes_count: 0,
        events: Default::default(),
        links: Default::default(),
        status: Status::Unset,
        instrumentation_scope: InstrumentationScope::builder("wire").build(),
    }
}

/// The scenario the parent chose, read from the environment it was started with.
#[test]
fn child() {
    let Ok(endpoint) = std::env::var(CHILD) else {
        return;
    };
    let options = OtelOptions {
        endpoint: Some(endpoint),
        headers: std::env::var("ULO_OTLP_CHILD_HEADERS").ok().map(Secret::new),
        ..OtelOptions::default()
    };
    let mut exporter = OtlpExporter::http();
    match std::env::var("ULO_OTLP_CHILD_PROXY").as_deref() {
        Ok("off") => exporter = exporter.no_proxy(),
        Ok(url) if url.starts_with("http") => exporter = exporter.proxy(url),
        _ => {}
    }
    let exporter = exporter.build(&options).unwrap();

    // The client blocks inside its first poll; a retry backoff after a refusal is not waited for.
    let (done, wait) = mpsc::channel();
    std::thread::spawn(move || {
        let mut fut = std::pin::pin!(exporter.export(vec![span()]));
        let mut cx = Context::from_waker(Waker::noop());
        while fut.as_mut().poll(&mut cx).is_pending() {
            std::thread::sleep(Duration::from_millis(5));
        }
        let _ = done.send(());
    });
    let _ = wait.recv_timeout(Duration::from_secs(4));
}

#[test]
fn an_explicit_header_replaces_one_from_the_environment() {
    let target = listen(OK);
    run_child(&[
        (CHILD, &target.url()),
        ("ULO_OTLP_CHILD_HEADERS", "x-ulo-key=explicit, x-ulo-enc=a%20b"),
        ("OTEL_EXPORTER_OTLP_HEADERS", "x-ulo-key=from-env,x-general=g"),
        (
            "OTEL_EXPORTER_OTLP_TRACES_HEADERS",
            "x-ulo-key=from-traces-env,x-ulo-enc=env-enc,x-env-only=e",
        ),
    ]);
    let head = target.head().expect("the receiver was reached");
    assert_eq!(header_values(&head, "x-ulo-key"), ["explicit"]);
    assert_eq!(header_values(&head, "x-ulo-enc"), ["a b"]);
    // A header the options do not name still comes from the environment.
    assert_eq!(header_values(&head, "x-env-only"), ["e"]);
}

#[test]
fn an_environment_header_alone_reaches_the_wire() {
    let target = listen(OK);
    run_child(&[(CHILD, &target.url()), ("OTEL_EXPORTER_OTLP_HEADERS", "x-ulo-key=from-env")]);
    let head = target.head().expect("the receiver was reached");
    assert_eq!(header_values(&head, "x-ulo-key"), ["from-env"]);
}

#[test]
fn the_proxy_variables_are_read_by_default() {
    let target = listen(OK);
    let env_proxy = listen(REFUSE);
    run_child(&[(CHILD, "http://otlp.invalid"), ("HTTP_PROXY", &env_proxy.url()), ("ALL_PROXY", &env_proxy.url())]);
    assert!(env_proxy.head().is_some(), "the environment's proxy was not used");
    assert!(target.head().is_none());
}

#[test]
fn no_proxy_ignores_the_proxy_variables() {
    let target = listen(OK);
    let env_proxy = listen(REFUSE);
    run_child(&[
        (CHILD, &target.url()),
        ("ULO_OTLP_CHILD_PROXY", "off"),
        ("HTTP_PROXY", &env_proxy.url()),
        ("HTTPS_PROXY", &env_proxy.url()),
        ("ALL_PROXY", &env_proxy.url()),
    ]);
    let head = target.head().expect("the receiver was reached directly");
    assert!(head.starts_with("POST /v1/traces"), "{head}");
    assert!(env_proxy.head().is_none(), "the environment's proxy was used");
}

#[test]
fn an_explicit_proxy_wins_over_the_environment() {
    let explicit = listen(REFUSE);
    let env_proxy = listen(REFUSE);
    run_child(&[
        (CHILD, "http://otlp.invalid"),
        ("ULO_OTLP_CHILD_PROXY", &explicit.url()),
        ("HTTP_PROXY", &env_proxy.url()),
        ("ALL_PROXY", &env_proxy.url()),
    ]);
    let head = explicit.head().expect("the explicit proxy was used");
    assert!(head.contains("otlp.invalid"), "{head}");
    assert!(env_proxy.head().is_none(), "the environment's proxy was used");
}

/// An `https` endpoint is spoken TLS to: the receiver's first bytes are a TLS handshake record of
/// major version 3, not a request line. Planted break: reqwest built with no TLS backend, which
/// writes `POST /v1/traces` in plaintext to an `https` endpoint.
#[test]
fn an_https_endpoint_is_spoken_tls_to() {
    let target = listen(OK);
    run_child(&[(CHILD, &format!("https://{}", target.addr)), ("ULO_OTLP_CHILD_PROXY", "off")]);
    let head = target.head().expect("the receiver was reached");
    assert_eq!(head.as_bytes().get(..2), Some(&[TLS_HANDSHAKE, 3][..]), "{head:?}");
}

/// An `https` endpoint goes through the explicit proxy as a tunnel, whatever `HTTPS_PROXY` says.
/// Planted break: the explicit proxy registered for `http` URLs alone, which every other proxy test
/// here leaves standing because their endpoints are `http`.
#[test]
fn an_explicit_proxy_tunnels_an_https_endpoint() {
    let explicit = listen(REFUSE);
    let env_proxy = listen(REFUSE);
    run_child(&[
        (CHILD, "https://otlp.invalid"),
        ("ULO_OTLP_CHILD_PROXY", &explicit.url()),
        ("HTTPS_PROXY", &env_proxy.url()),
        ("ALL_PROXY", &env_proxy.url()),
    ]);
    let head = explicit.head().expect("the explicit proxy was used");
    assert!(head.starts_with("CONNECT otlp.invalid:443 "), "{head}");
    assert!(env_proxy.head().is_none(), "the environment's proxy was used");
}

#[test]
fn a_malformed_proxy_url_is_refused_at_build() {
    let refusal = OtlpExporter::http().proxy("not a url").build(&OtelOptions::default());
    assert!(refusal.is_err());
}

/// The proxy URL may carry credentials, and an exporter is `Debug`. Planted break: the proxy
/// choice's derived `Debug`.
#[test]
fn a_proxy_s_credentials_are_not_printed() {
    let exporter = OtlpExporter::http().proxy("http://user:hunter2-sentinel@proxy.invalid:8080");
    let printed = format!("{exporter:?}");
    assert!(!printed.contains("hunter2-sentinel"), "{printed}");
}

/// A percent-encoded CR LF decodes to a value no header may hold, so the build refuses it rather
/// than writing a second header line.
#[test]
fn a_header_value_decoding_to_a_line_break_is_refused() {
    let options = OtelOptions {
        endpoint: Some("http://127.0.0.1:9".to_owned()),
        headers: Some(Secret::new("x-ulo-key=a%0D%0Ainjected: 1".to_owned())),
        ..OtelOptions::default()
    };
    assert!(OtlpExporter::http().build(&options).is_err());
}
