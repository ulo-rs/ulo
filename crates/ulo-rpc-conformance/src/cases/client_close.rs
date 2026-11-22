//! Scenarios: closing the client.

use std::time::{Duration, Instant};

use ulo_transport::ErrorKind;

use futures_util::FutureExt;

use crate::Broker;
use crate::cases::app::{ADD, Add, Fixture, NEVER, Sum, eventually, failed, joined, within};
use crate::rt::{sleep, spawn};

/// The waiting call's own timeout: longer than every bound below, so a call that fails within them
/// failed for the close.
const CALL_TIMEOUT: Duration = Duration::from_secs(60);

/// How long the client's close has to show: at the waiting call, and at the environment's count of
/// the client's connections.
const PATIENCE: Duration = Duration::from_secs(5);

/// The client app's close, bounded so a close that hangs fails the scenario. A destroy hook that
/// outlasts its own bound, `hook_timeout`, fails it sooner, as a failure in the close's report.
const CLOSE_BOUND: Duration = Duration::from_secs(30);

/// Closing the client's app while a call waits on a handler that never answers reports no failure
/// and fails the call `Unavailable` within `PATIENCE`, the client module's destroy hook having
/// closed the link. Where the environment counts the client's connections
/// (`Broker::client_connections`), every one ends within `PATIENCE` and none opens again while the
/// client makes no call. A call the same client makes afterwards fails `Unavailable` with "client
/// closed" and opens no connection: a closed client stays closed.
///
/// The call is shown to be waiting at its handler, and still unanswered, before the close, so the
/// scenario fails on a close that leaves it waiting, the caller's own timeout lying beyond every
/// bound here, and on a call that failed for another reason first.
pub async fn client_close<B: Broker>() {
    let Fixture { broker, server, client } = Fixture::<B>::start().await;
    let budget = broker.budget();
    let rpc = client.rpc.clone();

    let caller = rpc.clone();
    let mut waiting = spawn::<B, _, _>(async move { caller.request::<_, ()>(NEVER, &()).timeout(CALL_TIMEOUT).await });
    let probe = server.probe.clone();
    assert!(
        eventually::<B>(budget.settle + PATIENCE, || probe.unanswered() > 0).await,
        "the call did not reach its handler within {:?}",
        budget.settle + PATIENCE,
    );
    let held = broker.client_connections().await;
    if let Some(open) = held {
        assert!(open > 0, "the environment counts no connection of the client's while its call waits");
    }
    if let Some(ended) = (&mut waiting).now_or_never() {
        panic!("the call ended before its client closed: {ended:?}");
    }

    let closed = within::<B, _>(CLOSE_BOUND, "the client app's close", client.closed()).await;
    if let Err(error) = closed {
        panic!("the client app's close reported a failure: {error}");
    }
    let ended = joined(within::<B, _>(PATIENCE, "the call waiting when its client closed", waiting).await, "the waiting call's task");
    failed(ended, ErrorKind::Unavailable);

    if held.is_some() {
        let deadline = Instant::now() + PATIENCE;
        loop {
            match broker.client_connections().await {
                Some(0) => break,
                open if Instant::now() >= deadline => {
                    panic!("the client's connections were still open {PATIENCE:?} after its app closed: {open:?}")
                }
                _ => sleep::<B>(Duration::from_millis(50)).await,
            }
        }
        // A link reconnecting behind a closed client shows here: nothing has called since.
        sleep::<B>(budget.settle).await;
        let reopened = broker.client_connections().await;
        assert_eq!(reopened, Some(0), "the closed client opened a connection with no call made");
    }

    // A closed client stays closed: a call after the close is refused and opens no connection.
    let after = rpc.request::<_, Sum>(ADD, &Add { a: 2, b: 3 }).timeout(PATIENCE).await;
    let refused = failed(after, ErrorKind::Unavailable);
    assert_eq!((refused.message(), refused.reason()), ("client closed", Some("client_closed")), "a call after the client's close");
    if held.is_some() {
        let open = broker.client_connections().await;
        assert_eq!(open, Some(0), "a call after the client's close opened a connection");
    }
    server.stop().await;
}
