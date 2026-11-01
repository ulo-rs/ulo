use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use ulo::{ExecOptions, Execution, Signal};
use ulo_db::Store;

use crate::cases::{Client, Root, builder, connected, eventually, joined, sleep, spawn, store, timeout};
use crate::{Host, report};

/// The app's drain window: far longer than the queries the scenario holds across it.
const DRAIN: Duration = Duration::from_secs(10);

/// The pause between two of the execution's queries.
const GAP: Duration = Duration::from_millis(20);

/// The longest the scenario waits for the shutdown to begin draining once it has started it.
const PATIENCE: Duration = Duration::from_secs(30);

/// One query the execution made: whether it succeeded, and whether the app was draining when it
/// answered.
struct Query {
    outcome: Result<(), String>,
    draining: bool,
}

/// The pool closes after the transports drain, a query in flight at the drain completing. Planted
/// break: a pool closed before the drain.
///
/// No transport is bound, so the query runs in a standalone execution: `Execution::open` on the
/// root module, held by a task that queries the store through `Store::ping` on the module's own
/// client, a pause apart, from before the shutdown starts until the settle budget has passed since
/// the app began draining, and then drops the execution. The shutdown's drain waits for that
/// execution, as it waits for a transport's, and the store's close runs in the destroy step after
/// it. Every query succeeds, those answered during the drain among them; the execution ends on its
/// own inside the drain window, so the shutdown abandons nothing; and the shutdown reports no
/// failure, the store's close included. A pool closed at `Stopping`, or anywhere before the drain,
/// fails the queries made during it.
pub async fn after_the_drain<H: Host>() {
    let host = H::start().await;
    let hold = host.budget().settle;
    let app = connected(builder::<H>(Root::new().import(store::<H>(host.options()))).drain_timeout(DRAIN), "the app").await;
    let client: Client<H> = match app.get::<Client<H>>().await {
        Ok(client) => (*client).clone(),
        Err(error) => crate::startup_failed!("the app exports no client for `{}`: {error}", <H::Store as Store>::NAME),
    };
    let exec = Execution::open(&app.handle().root(), ExecOptions::new())
        .unwrap_or_else(|closed| crate::startup_failed!("a standalone execution was refused before the shutdown: {closed}"));

    let made = Arc::new(AtomicUsize::new(0));
    let querying = spawn::<H, _, _>({
        let made = Arc::clone(&made);
        async move {
            let began = Instant::now();
            let mut queries = Vec::new();
            let mut drained_at: Option<Instant> = None;
            loop {
                let outcome = <H::Store as Store>::ping(&client).await.map_err(|error| report(&*error));
                let draining = exec.is_draining();
                queries.push(Query { outcome, draining });
                made.fetch_add(1, Ordering::SeqCst);
                let now = Instant::now();
                if draining {
                    let since = *drained_at.get_or_insert(now);
                    if now - since >= hold {
                        break;
                    }
                }
                if now - began >= PATIENCE {
                    break;
                }
                sleep::<H>(GAP).await;
            }
            drop(exec);
            queries
        }
    });

    let first = eventually::<H>(hold.max(Duration::from_secs(5)), || made.load(Ordering::SeqCst) > 0).await;
    assert!(first, "the execution made no query before the shutdown");
    let shutdown = timeout::<H, _>(PATIENCE + DRAIN, app.close(Signal::new("conformance")))
        .await
        .unwrap_or_else(|| panic!("the shutdown did not finish within {:?}", PATIENCE + DRAIN));
    let queries = joined(querying.await, "the querying execution");

    let failed: Vec<(usize, &Query)> = queries.iter().enumerate().filter(|(_, query)| query.outcome.is_err()).collect();
    assert!(
        failed.is_empty(),
        "{} of {} queries failed; the first, query {}{}: {}",
        failed.len(),
        queries.len(),
        failed[0].0,
        if failed[0].1.draining { ", made during the drain" } else { ", made before the drain" },
        failed[0].1.outcome.as_ref().err().map_or("", String::as_str)
    );
    let during = queries.iter().filter(|query| query.draining).count();
    assert!(
        during > 0,
        "none of the execution's {} queries answered during the drain: the shutdown did not begin draining within {PATIENCE:?}",
        queries.len()
    );
    match shutdown {
        Ok(report) => assert_eq!(
            report.abandoned, 0,
            "the shutdown abandoned the querying execution, which ended {hold:?} into a {DRAIN:?} drain"
        ),
        Err(error) => panic!("the shutdown after the drain reported failures, the store's close among them: {error}"),
    }
}
