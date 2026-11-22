//! Checker (Q34-50): the link's publisher connection makes one attempt. A server that accepts and
//! closes each connection at once fails the client's connect after one accept, where redis's own
//! retries would dial it again up to six times.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use tokio::net::TcpListener;
use ulo::Runtime;
use ulo_rpc::{ConnectTimedOut, Link};
use ulo_rpc_redis::Redis;
use ulo_runtime_tokio::Tokio;

#[tokio::test(flavor = "multi_thread")]
async fn a_connection_closed_at_once_is_dialled_once() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("binds");
    let url = format!("redis://{}", listener.local_addr().expect("an address"));
    let accepted = Arc::new(AtomicUsize::new(0));
    let counting = Arc::clone(&accepted);
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            counting.fetch_add(1, Ordering::SeqCst);
            drop(stream);
        }
    });
    let runtime: Arc<dyn Runtime> = Arc::new(Tokio::current());
    let outcome = tokio::time::timeout(Duration::from_secs(25), Redis::url(url).connect(&runtime)).await.expect("the connect ended within 25 s");
    let refused = outcome.err().expect("a server that closes every connection was connected to");
    assert!(refused.downcast_ref::<ConnectTimedOut>().is_none(), "ended by the link's bound: {refused}");
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(accepted.load(Ordering::SeqCst), 1, "the publisher's connection was dialled more than once: {refused}");
}
