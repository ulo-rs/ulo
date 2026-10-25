//! The accept loop through its plug point: `Serve` over listeners of the tests' own, on smol, so
//! nothing here is the tokio listener's or the smol listener's. A connection reaches the closure
//! with its addresses and is served through hyper over `FuturesIo`; a handshake past its timeout
//! is dropped on the app's clock; the drain stops accepting and waits for the connections; the
//! accept loop and every connection are tasks of the runtime `Serve` was given; and `close_after`
//! polls each connection in its own first poll, and aborts them when dropped after it.

mod support;

use std::convert::Infallible;
use std::future::pending;
use std::io;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use async_net::{TcpListener, TcpStream};
use bytes::Bytes;
use http_body_util::{BodyExt, Empty, Full};
use hyper::server::conn::http1;
use ulo::{Runtime, Spawn};
use ulo_http::{ConnInfo, TlsInfo};
use ulo_server_hyper::{Accepted, FuturesIo, Incoming, Listener, ReadCount, Serve, ServeConfig};
use ulo_net::rustls::ServerConfig;
use ulo_net::{BoundListener, Endpoint};

use support::{Counting, PATIENCE, on_smol, within};

/// A listener over `async-net` whose connections need no handshake.
struct Plain {
    tcp: TcpListener,
}

impl Listener for Plain {
    type Io = FuturesIo<TcpStream>;

    fn adopt(listener: BoundListener, _tls: Option<Arc<ServerConfig>>) -> io::Result<Self> {
        Ok(Plain { tcp: TcpListener::try_from(listener.into_std())? })
    }

    async fn accept(&self) -> io::Result<Incoming<Self::Io>> {
        let (stream, peer) = self.tcp.accept().await?;
        let local = stream.local_addr().ok();
        Ok(Incoming::plain(FuturesIo::new(stream), peer, local))
    }
}

/// Handshakes [`Hanging`] started whose futures have since been dropped.
static ABANDONED: AtomicUsize = AtomicUsize::new(0);

struct Abandoned;

impl Drop for Abandoned {
    fn drop(&mut self) {
        ABANDONED.fetch_add(1, Ordering::SeqCst);
    }
}

/// A listener whose handshake never finishes.
struct Hanging {
    tcp: TcpListener,
}

impl Listener for Hanging {
    type Io = FuturesIo<TcpStream>;

    fn adopt(listener: BoundListener, _tls: Option<Arc<ServerConfig>>) -> io::Result<Self> {
        Ok(Hanging { tcp: TcpListener::try_from(listener.into_std())? })
    }

    async fn accept(&self) -> io::Result<Incoming<Self::Io>> {
        let (stream, peer) = self.tcp.accept().await?;
        Ok(Incoming::handshaking(peer, None, async move {
            let _abandoned = Abandoned;
            let _stream = stream;
            pending::<io::Result<(FuturesIo<TcpStream>, TlsInfo)>>().await
        }))
    }
}

/// A listener on a port the OS chose.
fn port_zero() -> BoundListener {
    BoundListener::bind(&Endpoint::parse("127.0.0.1:0").expect("an address")).expect("a port")
}

/// Serves one HTTP/1.1 request on `accepted`, answered `served`, until the connection ends.
async fn answer<L: Listener>(accepted: Accepted<L>) {
    let service = hyper::service::service_fn(|_req| async { Ok::<_, Infallible>(http::Response::new(Full::new(Bytes::from("served")))) });
    let _ = http1::Builder::new().serve_connection(accepted.io, service).await;
}

/// `GET /` on a connection of its own, its body read to the end.
async fn get(runtime: &Arc<dyn Runtime>, stream: TcpStream) -> (u16, Bytes) {
    let (mut sender, connection) = hyper::client::conn::http1::handshake(FuturesIo::new(stream)).await.expect("the client's handshake");
    drop(runtime.spawn(Box::pin(async move {
        let _ = connection.await;
    })));
    let request = http::Request::get("/").header(http::header::HOST, "serve").body(Empty::<Bytes>::new()).expect("a request");
    let response = sender.send_request(request).await.expect("an answer");
    let status = response.status().as_u16();
    let body = response.into_body().collect().await.expect("the body").to_bytes();
    (status, body)
}

#[test]
fn a_connection_reaches_the_closure_with_its_addresses_and_is_served_through_hyper() {
    on_smol(|smol| async move {
        let runtime: Arc<dyn Runtime> = Arc::new(smol);
        let listener = port_zero();
        let addr = listener.local_addr();
        let read_count = ReadCount::default();
        let config = ServeConfig { handshake_timeout: Some(PATIENCE), read_count: read_count.clone() };
        let serve = Arc::new(Serve::<Plain>::new(vec![listener], None, &config, Arc::clone(&runtime)).expect("the listener is adopted"));
        let seen: Arc<Mutex<Option<ConnInfo>>> = Arc::default();
        let running = {
            let (serve, seen) = (Arc::clone(&serve), Arc::clone(&seen));
            runtime.spawn(Box::pin(async move {
                let _ = serve
                    .run(move |accepted: Accepted<Plain>| {
                        *seen.lock().unwrap_or_else(PoisonError::into_inner) = Some(accepted.conn.clone());
                        answer(accepted)
                    })
                    .await;
            }))
        };

        let stream = TcpStream::connect(addr).await.expect("the server accepts");
        let client = stream.local_addr().expect("the client's address");
        assert_eq!(within("the answer", get(&runtime, stream)).await, (200, Bytes::from("served")));
        let conn = seen.lock().unwrap_or_else(PoisonError::into_inner).clone().expect("the closure was handed the connection");
        assert_eq!(conn.peer, Some(client), "the peer is the client's address");
        assert_eq!(conn.local, Some(addr), "the local address is the listener's");
        assert!(conn.tls.is_none(), "a connection with no handshake reports no TLS");
        assert_eq!(conn.version, http::Version::HTTP_11);
        assert_eq!(read_count.get(), 1, "the connection is counted at its first read");

        within("the drain", serve.drain()).await;
        assert!(within("the serve loop's end", running).await.is_finished());
    });
}

#[test]
fn a_handshake_past_its_timeout_is_dropped_on_the_app_clock_and_never_served() {
    on_smol(|smol| async move {
        let runtime: Arc<dyn Runtime> = Arc::new(smol);
        let listener = port_zero();
        let addr = listener.local_addr();
        let timeout = Duration::from_millis(150);
        let config = ServeConfig { handshake_timeout: Some(timeout), ..ServeConfig::default() };
        let serve = Arc::new(Serve::<Hanging>::new(vec![listener], None, &config, Arc::clone(&runtime)).expect("the listener is adopted"));
        let served = Arc::new(AtomicBool::new(false));
        let running = {
            let (serve, served) = (Arc::clone(&serve), Arc::clone(&served));
            runtime.spawn(Box::pin(async move {
                let _ = serve
                    .run(move |_accepted: Accepted<Hanging>| {
                        served.store(true, Ordering::SeqCst);
                        async {}
                    })
                    .await;
            }))
        };

        let before = ABANDONED.load(Ordering::SeqCst);
        let started = runtime.now();
        let _stream = TcpStream::connect(addr).await.expect("the server accepts");
        within("the hanging handshake's drop", async {
            while ABANDONED.load(Ordering::SeqCst) == before {
                runtime.sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        let took = runtime.now().saturating_duration_since(started);
        assert!(took >= timeout, "the handshake was dropped after {took:?}, before its timeout of {timeout:?}");
        assert!(!served.load(Ordering::SeqCst), "a connection whose handshake never finished reached the closure");

        within("the drain", serve.drain()).await;
        assert!(within("the serve loop's end", running).await.is_finished());
    });
}

#[test]
fn the_drain_resolves_draining_stops_accepting_and_waits_for_the_connections() {
    on_smol(|smol| async move {
        let runtime: Arc<dyn Runtime> = Arc::new(smol);
        let listener = port_zero();
        let addr = listener.local_addr();
        let serve = Arc::new(Serve::<Plain>::new(vec![listener], None, &ServeConfig::default(), Arc::clone(&runtime)).expect("adopted"));
        let started = Arc::new(AtomicBool::new(false));
        let finished = Arc::new(AtomicBool::new(false));
        let running = {
            let (serve, started, finished, timer) = (Arc::clone(&serve), Arc::clone(&started), Arc::clone(&finished), Arc::clone(&runtime));
            runtime.spawn(Box::pin(async move {
                let _ = serve
                    .run(move |accepted: Accepted<Plain>| {
                        let (started, finished, timer) = (Arc::clone(&started), Arc::clone(&finished), Arc::clone(&timer));
                        async move {
                            let _io = accepted.io;
                            started.store(true, Ordering::SeqCst);
                            accepted.draining.wait().await;
                            assert!(accepted.draining.is_draining());
                            // The connection's graceful shutdown, which the drain waits for.
                            timer.sleep(Duration::from_millis(200)).await;
                            finished.store(true, Ordering::SeqCst);
                        }
                    })
                    .await;
            }))
        };

        let _held = TcpStream::connect(addr).await.expect("the server accepts");
        within("the connection reaching the closure", async {
            while !started.load(Ordering::SeqCst) {
                runtime.sleep(Duration::from_millis(5)).await;
            }
        })
        .await;
        within("the drain", serve.drain()).await;
        assert!(finished.load(Ordering::SeqCst), "the drain returned before the connection it was waiting for had ended");
        let refused = TcpStream::connect(addr).await;
        assert!(refused.is_err(), "a connection after the drain was accepted: the listener is still open");
        assert!(within("the serve loop's end", running).await.is_finished());
    });
}

#[test]
fn the_accept_loop_and_every_connection_are_tasks_of_the_runtime_serve_was_given() {
    on_smol(|smol| async move {
        let counting = Counting::new(smol);
        let runtime: Arc<dyn Runtime> = Arc::new(counting.clone());
        let listener = port_zero();
        let addr = listener.local_addr();
        let serve = Arc::new(Serve::<Plain>::new(vec![listener], None, &ServeConfig::default(), Arc::clone(&runtime)).expect("adopted"));
        let running = {
            let serve = Arc::clone(&serve);
            counting.smol.spawn(Box::pin(async move {
                let _ = serve.run(answer::<Plain>).await;
            }))
        };
        let waited = |count: usize, what: &'static str| {
            let (counting, timer) = (counting.clone(), Arc::clone(&runtime));
            async move {
                within(what, async {
                    while counting.spawned() < count {
                        timer.sleep(Duration::from_millis(5)).await;
                    }
                })
                .await;
            }
        };
        waited(1, "the accept loop's task").await;
        assert_eq!(counting.spawned(), 1, "the accept loop, spawned on the runtime");
        let first = TcpStream::connect(addr).await.expect("the server accepts");
        let second = TcpStream::connect(addr).await.expect("the server accepts");
        waited(3, "both connections' tasks").await;
        assert_eq!(counting.spawned(), 3, "one task per connection, spawned on the runtime");
        drop((first, second));

        within("the drain", serve.drain()).await;
        assert!(within("the serve loop's end", running).await.is_finished());
    });
}

/// `close_after` polls `close` once and answers whether it ended there, as the core polls a
/// `close` under a spent shutdown cap; the future is dropped as it returns.
async fn first_poll_of_close(serve: &Serve<Plain>) -> bool {
    let mut close = Box::pin(serve.close_after(pending::<()>()));
    std::future::poll_fn(|cx| std::task::Poll::Ready(close.as_mut().poll(cx).is_ready())).await
}

/// A connection whose future is ready to answer once `ready` is set, which wakes nothing: only a
/// poll it did not ask for finds it. It records each poll that answered.
fn answering_when_polled(ready: Arc<AtomicBool>, answered: Arc<AtomicUsize>) -> impl Future<Output = ()> + Send {
    std::future::poll_fn(move |_| {
        if ready.load(Ordering::SeqCst) {
            answered.fetch_add(1, Ordering::SeqCst);
            std::task::Poll::Ready(())
        } else {
            std::task::Poll::Pending
        }
    })
}

#[test]
fn close_afters_first_poll_polls_each_connection_once_on_the_calling_thread() {
    on_smol(|smol| async move {
        let runtime: Arc<dyn Runtime> = Arc::new(smol);
        let listener = port_zero();
        let addr = listener.local_addr();
        let serve = Arc::new(Serve::<Plain>::new(vec![listener], None, &ServeConfig::default(), Arc::clone(&runtime)).expect("adopted"));
        let (ready, answered, started) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicUsize::new(0)), Arc::new(AtomicBool::new(false)));
        let running = {
            let (serve, ready, answered, started) = (Arc::clone(&serve), Arc::clone(&ready), Arc::clone(&answered), Arc::clone(&started));
            runtime.spawn(Box::pin(async move {
                let _ = serve
                    .run(move |accepted: Accepted<Plain>| {
                        let (ready, answered, started) = (Arc::clone(&ready), Arc::clone(&answered), Arc::clone(&started));
                        async move {
                            let _io = accepted.io;
                            started.store(true, Ordering::SeqCst);
                            answering_when_polled(ready, answered).await;
                        }
                    })
                    .await;
            }))
        };
        let _held = TcpStream::connect(addr).await.expect("the server accepts");
        within("the connection reaching the closure", async {
            while !started.load(Ordering::SeqCst) {
                runtime.sleep(Duration::from_millis(5)).await;
            }
        })
        .await;

        ready.store(true, Ordering::SeqCst);
        assert!(!first_poll_of_close(&serve).await, "close ended on its first poll with a connection open");
        assert_eq!(answered.load(Ordering::SeqCst), 1, "the connection was not polled in close's first poll");
        assert!(within("the serve loop's end", running).await.is_finished());
    });
}

/// A connection that never ends, recording its future's drop.
struct Dropped(Arc<AtomicBool>);

impl Drop for Dropped {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

#[test]
fn a_close_after_dropped_after_its_first_poll_still_aborts_the_connections() {
    on_smol(|smol| async move {
        let runtime: Arc<dyn Runtime> = Arc::new(smol);
        let listener = port_zero();
        let addr = listener.local_addr();
        let serve = Arc::new(Serve::<Plain>::new(vec![listener], None, &ServeConfig::default(), Arc::clone(&runtime)).expect("adopted"));
        let (started, dropped) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
        let running = {
            let (serve, started, dropped) = (Arc::clone(&serve), Arc::clone(&started), Arc::clone(&dropped));
            runtime.spawn(Box::pin(async move {
                let _ = serve
                    .run(move |accepted: Accepted<Plain>| {
                        let (started, dropped) = (Arc::clone(&started), Arc::clone(&dropped));
                        async move {
                            let _io = accepted.io;
                            let _dropped = Dropped(dropped);
                            started.store(true, Ordering::SeqCst);
                            // Ignores the drain: only an abort ends it.
                            pending::<()>().await;
                        }
                    })
                    .await;
            }))
        };
        let _held = TcpStream::connect(addr).await.expect("the server accepts");
        within("the connection reaching the closure", async {
            while !started.load(Ordering::SeqCst) {
                runtime.sleep(Duration::from_millis(5)).await;
            }
        })
        .await;

        assert!(!first_poll_of_close(&serve).await, "close ended on its first poll with a connection open");
        within("the connection's abort", async {
            while !dropped.load(Ordering::SeqCst) {
                runtime.sleep(Duration::from_millis(5)).await;
            }
        })
        .await;
        assert!(within("the serve loop's end", running).await.is_finished(), "the accept loop outlived the cut close");
    });
}

#[test]
fn close_after_aborts_a_connection_that_outlives_its_grace_and_returns_at_once_with_none_open() {
    on_smol(|smol| async move {
        let runtime: Arc<dyn Runtime> = Arc::new(smol);
        let listener = port_zero();
        let addr = listener.local_addr();
        let serve = Arc::new(Serve::<Plain>::new(vec![listener], None, &ServeConfig::default(), Arc::clone(&runtime)).expect("adopted"));
        let (started, dropped) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
        let running = {
            let (serve, started, dropped) = (Arc::clone(&serve), Arc::clone(&started), Arc::clone(&dropped));
            runtime.spawn(Box::pin(async move {
                let _ = serve
                    .run(move |accepted: Accepted<Plain>| {
                        let (started, dropped) = (Arc::clone(&started), Arc::clone(&dropped));
                        async move {
                            let _io = accepted.io;
                            let _dropped = Dropped(dropped);
                            started.store(true, Ordering::SeqCst);
                            // Ignores the drain: only an abort ends it.
                            pending::<()>().await;
                        }
                    })
                    .await;
            }))
        };
        let _held = TcpStream::connect(addr).await.expect("the server accepts");
        within("the connection reaching the closure", async {
            while !started.load(Ordering::SeqCst) {
                runtime.sleep(Duration::from_millis(5)).await;
            }
        })
        .await;

        let begun = std::time::Instant::now();
        within("close_after under a 100 ms grace", serve.close_after(runtime.sleep(Duration::from_millis(100)))).await;
        let took = begun.elapsed();
        assert!(took >= Duration::from_millis(100), "close_after returned before its grace with a connection open: {took:?}");
        assert!(dropped.load(Ordering::SeqCst), "the connection outlived close_after");
        assert!(within("the serve loop's end", running).await.is_finished());

        // With no connection open, the drain's end ends the wait, whatever the grace.
        let listener = port_zero();
        let idle = Arc::new(Serve::<Plain>::new(vec![listener], None, &ServeConfig::default(), Arc::clone(&runtime)).expect("adopted"));
        let running = {
            let idle = Arc::clone(&idle);
            runtime.spawn(Box::pin(async move {
                let _ = idle.run(|accepted: Accepted<Plain>| async move { drop(accepted) }).await;
            }))
        };
        within("close_after with nothing open and a grace that never ends", idle.close_after(pending::<()>())).await;
        assert!(within("the idle serve loop's end", running).await.is_finished());
    });
}
