//! What the RPC examples' tests share. An RPC server has no HTTP request to make, so its binary
//! prints `listening on tcp://ADDR` and the test dials that address with the framework's own
//! client.

use std::future::IntoFuture;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

use crate::Bin;

/// Starts an RPC example's binary with `PORT` set to a port that was free a moment ago, as
/// [`Bin::server`] does for HTTP; the port is returned for [`tcp_addr`] to check.
pub fn spawn_tcp(exe: &str, envs: &[(&str, &str)]) -> (Bin, u16) {
    let port = std::net::TcpListener::bind("127.0.0.1:0").expect("a free port").local_addr().expect("its address").port();
    let port_text = port.to_string();
    (Bin::spawn(exe, &[envs, &[("PORT", port_text.as_str())]].concat()), port)
}

/// Reads lines until the first `listening on tcp://ADDR` and returns `ADDR`, which has to be on
/// `port`.
pub fn tcp_addr(bin: &mut Bin, port: u16) -> SocketAddr {
    loop {
        if let Some(addr) = bin.next_line().strip_prefix("listening on tcp://") {
            let addr: SocketAddr = addr.parse().unwrap_or_else(|e| panic!("`{addr}` is not an address: {e}"));
            assert_eq!(addr.port(), port, "the binary was given PORT={port} and listens on {addr}");
            return addr;
        }
    }
}

/// Polls `ready` every few milliseconds until it holds, for an effect nothing answers for, such as
/// an event's handler. Panics after ten seconds of real time.
pub async fn eventually(what: &str, mut ready: impl FnMut() -> bool) {
    let started = Instant::now();
    while !ready() {
        assert!(started.elapsed() < Duration::from_secs(10), "{what} did not happen within ten seconds");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// Awaits a call, or any future, for at most ten seconds of real time, so a call that never answers fails the test
/// instead of hanging it.
pub async fn bounded<F: IntoFuture>(fut: F) -> F::Output {
    tokio::time::timeout(Duration::from_secs(10), fut.into_future()).await.expect("the call answered within ten seconds")
}
