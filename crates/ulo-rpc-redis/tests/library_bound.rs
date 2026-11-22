//! Q34-50: against a server that accepts the connection and never answers, `connect_within` at
//! its 30-second default is the bound that ends a server's `listen` and a client's connect, not
//! the redis client's own, which the link sets one second past it with no retry. Each test runs
//! on a paused tokio clock, which jumps to the next timer whenever the runtime has nothing to run,
//! so the shorter of the two bounds is the one that ends the wait, with no real time spent on
//! either. Planted break: the redis client at its own defaults, 1 s per attempt retried six
//! times, which ends the wait after about 16 s with its own error.

use std::sync::Arc;
use std::time::Duration;

use tokio::net::TcpListener;
use ulo::{App, BoxError, Module, ModuleDef, ModuleIdentity, Runtime};
use ulo_rpc::{ConnectTimedOut, Link, Pattern};
use ulo_rpc_redis::Redis;
use ulo_runtime_tokio::Tokio;

/// `connect_within` unset.
const DEFAULT_WITHIN: Duration = Duration::from_secs(30);

struct Root;

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        let _ = m;
    }
}

/// A socket that accepts every connection and holds it open, writing nothing.
async fn silent() -> (String, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("the silent server binds");
    let url = format!("redis://{}", listener.local_addr().expect("the silent server has an address"));
    let held = tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((stream, _)) = listener.accept().await {
            held.push(stream);
        }
    });
    (url, held)
}

/// `outcome` is the link's own `ConnectTimedOut` at its default bound, reached at that bound on
/// the paused clock.
fn cut_at_the_default(outcome: Result<(), BoxError>, waited: Duration, what: &str) {
    let refused = outcome.err().unwrap_or_else(|| panic!("{what} connected to a server that never answers"));
    let timed_out = refused
        .downcast_ref::<ConnectTimedOut>()
        .unwrap_or_else(|| panic!("{what} ended other than as `ConnectTimedOut`, after {waited:?}: {refused}"));
    assert_eq!(timed_out.within(), DEFAULT_WITHIN, "{what}: {refused}");
    assert!((DEFAULT_WITHIN..DEFAULT_WITHIN + Duration::from_secs(1)).contains(&waited), "{what} ended after {waited:?}");
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn a_client_s_connect_to_a_silent_server_ends_at_connect_within() {
    let (url, _held) = silent().await;
    let runtime: Arc<dyn Runtime> = Arc::new(Tokio::current());
    let began = tokio::time::Instant::now();
    let outcome = Redis::url(url).connect(&runtime).await.map(drop);
    cut_at_the_default(outcome, began.elapsed(), "the client's connect");
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn a_server_s_listen_on_a_silent_server_ends_at_connect_within() {
    let (url, _held) = silent().await;
    let app = App::builder(Root).runtime(Tokio::current()).wire().expect("the app wires").connect().await.expect("the app connects");
    let mut link = Redis::url(url);
    link.prepare(&app.handle()).await.expect("the link prepares");
    let began = tokio::time::Instant::now();
    let outcome = link.listen(&[Pattern::from("library_bound.add").into()]).await.map(drop);
    cut_at_the_default(outcome, began.elapsed(), "`listen`");
}
