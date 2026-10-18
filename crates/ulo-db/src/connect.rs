use std::sync::Arc;
use std::time::Duration;

use ulo::{BoxError, BoxFuture, Runtime};

use crate::error::UnsupportedRuntime;
use crate::store::Store;

/// The retry policy of one module.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Policy {
    /// The total connect bound.
    pub(crate) connect_within: Duration,
    pub(crate) retries: u32,
    pub(crate) backoff: Duration,
}

/// Compares the given runtime's `Spawn::name` with `"tokio"` before anything is built. A comparison
/// and no ambient lookup, so a smol app driven from a thread with a tokio context still answers
/// `"smol"`. `"unknown"` is refused like any other name.
pub(crate) fn require_tokio(runtime: &dyn Runtime, module: &'static str) -> Result<(), UnsupportedRuntime> {
    let _ = (runtime, module);
    todo!()
}

/// The pool's factory: the name check, then `S::create` through `spawn_with`.
///
/// For a store whose `create` connects, the loop around `create` is the store's one retry loop:
/// the module's retries and backoff, ending on `Permanent`, under the construction bound
/// `connect_within`. For any other store a single `create` runs and `.ready` retries the ping.
pub(crate) async fn create<S: Store>(
    runtime: Arc<dyn Runtime>,
    options: S::Options,
    policy: Policy,
) -> Result<S::Client, BoxError> {
    let _ = (runtime, options, policy);
    todo!()
}

/// `S::ping` through `spawn_with`.
pub(crate) fn ping<S: Store>(runtime: Arc<dyn Runtime>, client: S::Client) -> BoxFuture<'static, Result<(), BoxError>> {
    let _ = (runtime, client);
    todo!()
}

/// `S::close` through `spawn_with`.
pub(crate) fn close<S: Store>(runtime: Arc<dyn Runtime>, client: S::Client) -> BoxFuture<'static, ()> {
    let _ = (runtime, client);
    todo!()
}
