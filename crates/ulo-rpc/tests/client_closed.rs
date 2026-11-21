//! Q35-12: a closed client stays closed. After `RpcClient::close`, or `RpcClientModule`'s destroy
//! hook, every call on the client and its clones fails `Unavailable` with "client closed" and
//! connects nothing; a call already past its connect is answered as before. The link here counts
//! its connects and closes and answers only what the test feeds it.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use futures_util::stream;
use tokio::sync::mpsc;
use ulo::{App, BoxError, Module, ModuleDef, ModuleIdentity, Runtime, Signal};
use ulo_rpc::link::Inbound;
use ulo_rpc::{Capabilities, Data, DeliveryMode, ErrorKind, Frame, Link, Outbound, RpcClient, RpcClientModule, RpcError, Subscription};
use ulo_runtime_tokio::Tokio;

/// How long a gated wait lasts before the test fails.
const PATIENCE: Duration = Duration::from_secs(5);

#[derive(Clone, Default)]
struct Counted {
    connects: Arc<AtomicUsize>,
    closes: Arc<AtomicUsize>,
    sent: Arc<Mutex<Vec<Frame>>>,
    replies: Arc<Mutex<Option<mpsc::UnboundedSender<Frame>>>>,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Link for Counted {
    const NAME: &'static str = "counted";

    fn capabilities(&self) -> Capabilities {
        Capabilities::new(DeliveryMode::Addressed)
    }

    async fn listen(&self, _: &[Subscription]) -> Result<Inbound, BoxError> {
        Err("the counted link serves nothing".into())
    }

    async fn connect(&self, _: &Arc<dyn Runtime>) -> Result<Outbound, BoxError> {
        self.connects.fetch_add(1, Ordering::SeqCst);
        let (feed, replies) = mpsc::unbounded_channel();
        *lock(&self.replies) = Some(feed);
        let sent = Arc::clone(&self.sent);
        Ok(Outbound {
            send: Box::new(move |_, frame, _| {
                lock(&sent).push(frame);
                Box::pin(async { Ok(()) })
            }),
            replies: Box::pin(stream::unfold(replies, |mut replies| async move { replies.recv().await.map(|frame| (frame, replies)) })),
        })
    }

    async fn drain(&self) {}

    async fn close(&self) -> Result<(), BoxError> {
        self.closes.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

fn client_closed<T: std::fmt::Debug>(outcome: Result<T, RpcError>, what: &str) {
    let error = outcome.expect_err(&format!("{what} on a closed client was answered"));
    assert_eq!((error.kind(), error.message(), error.reason()), (ErrorKind::Unavailable, "client closed", Some("client_closed")), "{what}");
}

/// Every call kind is refused after `close`, and none connects. Planted break: the closed flag
/// read nowhere, which connects for each call.
#[tokio::test]
async fn every_call_after_close_fails_client_closed_and_connects_nothing() {
    let link = Counted::default();
    let client = RpcClient::new(link.clone(), Arc::new(Tokio::current())).expect("the link is usable");
    client.close().await.expect("the client closes");
    client_closed(client.request::<_, u32>("p", &1u32).await, "a request");
    client_closed(client.emit("p", &1u32).await, "an emit");
    client_closed(client.stream::<_, u32>("p", &1u32).await.map(drop), "a stream");
    client_closed(client.send_stream::<_, u32>("p", stream::iter([1u32])).await, "a streamed request");
    client_closed(client.duplex::<_, u32>("p", stream::iter([1u32])).await.map(drop), "a duplex");
    assert_eq!(link.connects.load(Ordering::SeqCst), 0, "a closed client connected");
    assert_eq!(link.closes.load(Ordering::SeqCst), 1, "`close` closes the link");
}

/// Every clone reads one flag, made before the close or after.
#[tokio::test]
async fn a_clone_of_a_closed_client_is_closed() {
    let link = Counted::default();
    let client = RpcClient::new(link.clone(), Arc::new(Tokio::current())).expect("the link is usable");
    let before = client.clone();
    client.close().await.expect("the client closes");
    let after = client.clone();
    client_closed(before.request::<_, u32>("p", &1u32).await, "a request on a clone made before the close");
    client_closed(after.request::<_, u32>("p", &1u32).await, "a request on a clone made after the close");
    assert_eq!(link.connects.load(Ordering::SeqCst), 0);
}

/// A second `close` closes the link again and keeps the client closed.
#[tokio::test]
async fn closing_twice_keeps_the_client_closed() {
    let link = Counted::default();
    let client = RpcClient::new(link.clone(), Arc::new(Tokio::current())).expect("the link is usable");
    client.close().await.expect("the first close");
    client.close().await.expect("the second close");
    assert_eq!(link.closes.load(Ordering::SeqCst), 2);
    client_closed(client.request::<_, u32>("p", &1u32).await, "a request after two closes");
    assert_eq!(link.connects.load(Ordering::SeqCst), 0);
}

/// A call already sent when the client closes is answered by its reply as before: the flag refuses
/// a call at its connect, and this link's close ends nothing. Planted break: `close` ending the
/// held connection, which fails the call waiting on it.
#[tokio::test]
async fn a_call_in_flight_at_close_is_answered() {
    let link = Counted::default();
    let client = RpcClient::new(link.clone(), Arc::new(Tokio::current())).expect("the link is usable");
    let calling = client.clone();
    let call = tokio::spawn(async move { calling.request::<_, u32>("p", &1u32).await });
    let id = tokio::time::timeout(PATIENCE, async {
        loop {
            if let Some(Frame::Req { id, .. }) = lock(&link.sent).first() {
                break *id;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the request was sent");
    client.close().await.expect("the client closes");
    let feed = lock(&link.replies).clone().expect("the connection's reply lane");
    feed.send(Frame::Res { id, data: Data::new(b"5".to_vec()) }).expect("the reply is fed");
    let answer = tokio::time::timeout(PATIENCE, call).await.expect("the call ends").expect("the call's task");
    assert_eq!(answer.expect("the call in flight is answered"), 5);
    client_closed(client.request::<_, u32>("p", &1u32).await, "a request after the close");
    assert_eq!(link.connects.load(Ordering::SeqCst), 1);
}

struct Root(Counted);

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.import(RpcClientModule::for_root(self.0.clone()));
    }
}

/// The module's destroy hook closes its client: a call made after the app closed fails "client
/// closed" and opens no connection to the broker. Planted break: the hook closing the link and not
/// the client, which connects again.
#[tokio::test]
async fn a_module_s_client_is_closed_by_its_app_s_close() {
    let link = Counted::default();
    let app = App::builder(Root(link.clone())).runtime(Tokio::current()).wire().expect("the app wires").connect().await.expect("the app connects");
    let client = (*app.get::<RpcClient>().await.expect("the app holds an `RpcClient`")).clone();
    app.close(Signal::new("test")).await.expect("the app closes");
    assert_eq!(link.closes.load(Ordering::SeqCst), 1, "the destroy hook closes the link");
    client_closed(client.request::<_, u32>("p", &1u32).await, "a request after the app's close");
    assert_eq!(link.connects.load(Ordering::SeqCst), 0, "a client that outlived its app connected");
}
