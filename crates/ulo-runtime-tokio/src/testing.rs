//! A test serves its app over a real socket on the tokio runtime it runs on: [`start`] is
//! `ulo::testing::TestServer::start_on` with the app's runtime set to [`Tokio::current`], as `run`
//! sets it. The servers come from the caller, so a gRPC-only or WebSocket-only app tests itself
//! with no HTTP crate:
//!
//! ```no_run
//! # use ulo::testing::TestApp;
//! # #[ulo::module]
//! # struct AppModule;
//! # async fn f(server: impl ulo::Server) -> Result<(), Box<dyn std::error::Error>> {
//! let served = ulo_runtime_tokio::testing::start(TestApp::of(AppModule), server).await?;
//! let addr = served.addr();
//! // ... connect a client to `addr` ...
//! served.close().await?;
//! # Ok(())
//! # }
//! ```
//!
//! `start` sets the runtime and leaves the clock to the app: a clock given with `TestApp::timer`
//! stays the app's clock, and without one the app times on tokio's.

use ulo::testing::{TestApp, TestServer};
use ulo::{Servers, StartupError};

use crate::Tokio;

/// Wires and connects `app` on the current tokio runtime, binds `servers` and serves them in a task
/// of that runtime, as [`TestServer::start_on`] does.
///
/// # Panics
///
/// Outside a tokio runtime, as [`Tokio::current`] does.
pub async fn start(app: TestApp, servers: impl Servers) -> Result<TestServer, StartupError> {
    TestServer::start_on(app.runtime(Tokio::current()), servers).await
}
