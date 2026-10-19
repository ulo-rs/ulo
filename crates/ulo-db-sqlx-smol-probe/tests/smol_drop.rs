//! SQLx built with `runtime-smol` beside `runtime-tokio`, as feature unification can build it in an
//! application: where does a `PoolConnection` dropped on a thread with no tokio context return to
//! its pool? SQLx tests `Handle::try_current()` at every spawn, so that thread has no tokio and the
//! return task goes to smol's global executor.
#![cfg(feature = "sqlite")]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use sqlx::sqlite::SqlitePoolOptions;

#[test]
fn a_connection_dropped_off_the_runtime_returns_on_smols_executor() {
    let released: Arc<Mutex<Vec<Option<String>>>> = Arc::default();
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    let seen = Arc::clone(&released);
    let connection = runtime.block_on(async move {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .after_release(move |_, _| {
                let seen = Arc::clone(&seen);
                Box::pin(async move {
                    seen.lock().unwrap().push(std::thread::current().name().map(str::to_owned));
                    Ok(true)
                })
            })
            .connect("sqlite::memory:")
            .await
            .expect("an in-memory pool");
        pool.acquire().await.expect("a connection")
    });

    // A plain thread: no tokio context, as in user code that moves a connection off the runtime.
    std::thread::spawn(move || drop(connection)).join().unwrap();

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while released.lock().unwrap().is_empty() && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    let names = released.lock().unwrap().clone();
    assert_eq!(names.len(), 1, "the connection was not returned to its pool: {names:?}");
    let name = names[0].clone().unwrap_or_default();
    assert!(name.starts_with("smol"), "the return ran on `{name}`, not on smol's executor: the premise in PROBES_db.md 1.2 no longer holds");
}
