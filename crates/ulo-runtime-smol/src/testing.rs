//! A test serves its app over a real socket on the smol executor it runs on: [`start`] is
//! `ulo::testing::TestServer::start_on` with the app's runtime set to the [`Smol`] whose
//! [`block_on`](Smol::block_on) the test is inside. The servers come from the caller, so an app on
//! any transport tests itself with the crates it already depends on:
//!
//! ```no_run
//! # use std::sync::Arc;
//! # use ulo::testing::TestApp;
//! # use ulo_runtime_smol::{Executor, Smol};
//! # #[ulo::module]
//! # struct AppModule;
//! # fn f(server: impl ulo::Server) {
//! let smol = Smol::new(Arc::new(Executor::new()));
//! smol.block_on(async {
//!     let served = ulo_runtime_smol::testing::start(TestApp::of(AppModule), server).await.unwrap();
//!     let addr = served.addr();
//!     // ... connect a client to `addr` ...
//!     served.close().await.unwrap();
//! });
//! # }
//! ```
//!
//! A smol test ends with `close`: the listener of a `TestServer` dropped without it is released only
//! when the executor next runs, which it never does after `block_on` returns.
//!
//! `start` sets the runtime and leaves the clock to the app: a clock given with `TestApp::timer`
//! stays the app's clock, and without one the app times on smol's.

use ulo::testing::{TestApp, TestServer};
use ulo::{Servers, StartupError};

use crate::Smol;

/// Wires and connects `app` on the smol runtime running the calling thread, binds `servers` and
/// serves them in a task of that runtime, as [`TestServer::start_on`] does.
///
/// # Panics
///
/// Outside [`Smol::block_on`], where [`Smol::current`] answers `None`.
pub async fn start(app: TestApp, servers: impl Servers) -> Result<TestServer, StartupError> {
    let smol = Smol::current().expect("`testing::start` runs inside `Smol::block_on`");
    TestServer::start_on(app.runtime(smol), servers).await
}
