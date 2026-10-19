//! An app serving a gateway over the Redis adapter, bound, broadcasting once and dropped without
//! `serve` or `close`, lets go of both its Redis connections once its last handle drops: the
//! hub's delivery task ends, the subscription's task with it, which closes the Pub/Sub
//! connection, and the publisher's connection drops with the adapter (F434).
//!
//! The Redis server is a stand-in on a local port answering the few commands the adapter sends,
//! counting the connections it holds open: no container runs.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use ulo::{App, Module, ModuleDef, ModuleIdentity, injectable, routes};
use ulo_runtime_tokio::Tokio;
use ulo_ws::{Rooms, WsModule};
use ulo_ws_redis::Redis;

/// How long a drop has to reach the stand-in.
const PATIENCE: Duration = Duration::from_secs(5);

/// A stand-in Redis server: what it has accepted and how many of those are still open.
#[derive(Default)]
struct Standin {
    accepted: AtomicUsize,
    open: AtomicUsize,
}

impl Standin {
    /// Listens on a local port, answering each connection on a task of its own.
    async fn start() -> (Arc<Standin>, String) {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("the stand-in binds");
        let url = format!("redis://{}", listener.local_addr().expect("the stand-in's address"));
        let standin = Arc::new(Standin::default());
        let serving = Arc::clone(&standin);
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                serving.accepted.fetch_add(1, Ordering::SeqCst);
                serving.open.fetch_add(1, Ordering::SeqCst);
                let serving = Arc::clone(&serving);
                tokio::spawn(async move {
                    let _ = answer(stream).await;
                    serving.open.fetch_sub(1, Ordering::SeqCst);
                });
            }
        });
        (standin, url)
    }

    fn accepted(&self) -> usize {
        self.accepted.load(Ordering::SeqCst)
    }

    fn open(&self) -> usize {
        self.open.load(Ordering::SeqCst)
    }
}

/// Answers RESP commands until the client hangs up: `SUBSCRIBE` and `PSUBSCRIBE` with their
/// confirmations, `PUBLISH` with no receiver, anything else `+OK`.
async fn answer(stream: TcpStream) -> std::io::Result<()> {
    let (read, mut write) = stream.into_split();
    let mut read = BufReader::new(read);
    loop {
        let Some(command) = command(&mut read).await? else { return Ok(()) };
        let reply = match command.first().map(|name| name.to_ascii_uppercase()) {
            Some(name) if name == b"SUBSCRIBE" || name == b"PSUBSCRIBE" => {
                let kind = String::from_utf8_lossy(&name).to_lowercase();
                let channel = String::from_utf8_lossy(command.get(1).map_or(&[][..], Vec::as_slice)).into_owned();
                format!("*3\r\n${}\r\n{kind}\r\n${}\r\n{channel}\r\n:1\r\n", kind.len(), channel.len())
            }
            Some(name) if name == b"PUBLISH" => ":0\r\n".to_owned(),
            _ => "+OK\r\n".to_owned(),
        };
        write.write_all(reply.as_bytes()).await?;
    }
}

/// One command as an array of bulk strings; `None` once the client has hung up.
async fn command(read: &mut BufReader<tokio::net::tcp::OwnedReadHalf>) -> std::io::Result<Option<Vec<Vec<u8>>>> {
    let mut line = String::new();
    if read.read_line(&mut line).await? == 0 {
        return Ok(None);
    }
    let count: usize = line.trim_end().trim_start_matches('*').parse().map_err(std::io::Error::other)?;
    let mut parts = Vec::with_capacity(count);
    for _ in 0..count {
        line.clear();
        read.read_line(&mut line).await?;
        let len: usize = line.trim_end().trim_start_matches('$').parse().map_err(std::io::Error::other)?;
        let mut part = vec![0; len + 2];
        read.read_exact(&mut part).await?;
        part.truncate(len);
        parts.push(part);
    }
    Ok(Some(parts))
}

#[injectable]
struct Members;

#[routes]
#[ulo_ws::gateway(path = "/members", port = own)]
impl Members {
    #[ulo_ws::message("ping")]
    fn ping(&self) -> &'static str {
        "pong"
    }
}

/// `WsModule` over the Redis adapter on `url`, and the one gateway.
struct Root(String);

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.import(WsModule::for_root().broadcast(Redis::url(&self.0)));
        m.controller::<Members>();
    }
}

/// Waits until `done` holds, failing naming `what` after [`PATIENCE`].
async fn until(what: &str, done: impl Fn() -> bool) {
    let deadline = Instant::now() + PATIENCE;
    while !done() {
        assert!(Instant::now() < deadline, "{what} did not happen within {PATIENCE:?}");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_app_dropped_unclosed_closes_its_redis_connections() {
    let (standin, url) = Standin::start().await;
    let app = App::builder(Root(url))
        .runtime(Tokio::current())
        .wire()
        .unwrap_or_else(|error| panic!("the app did not wire: {error}"))
        .connect()
        .await
        .unwrap_or_else(|error| panic!("the app did not connect: {error}"))
        .bind(ulo_ws_hyper::Server::new("127.0.0.1:0"))
        .listen()
        .await
        .unwrap_or_else(|error| panic!("the app did not listen: {error}"));
    until("the subscription's connection", || standin.open() == 1).await;
    let handle = app.handle();
    let rooms = handle.get::<Rooms>().await.unwrap_or_else(|error| panic!("`Rooms` did not resolve: {error}"));
    rooms.to_all().emit("notice", &1).await.unwrap_or_else(|error| panic!("the broadcast failed: {error}"));
    drop(rooms);
    assert_eq!((standin.accepted(), standin.open()), (2, 2), "the subscription's and the publisher's connections");
    drop(app);
    drop(handle);
    until("both connections' close", || standin.open() == 0).await;
}
