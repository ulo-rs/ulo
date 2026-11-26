use std::net::SocketAddr;

use crate::app::AppHandle;
use crate::error::{Shutdown, ShutdownError, StartupError};
use crate::runtime::{TaskEnd, ValueHandle, spawn_with};
use crate::signal::Signal;
use crate::testing::TestApp;
use crate::{BoundAddr, Servers};

/// An app bound and serving, started by [`start_on`](Self::start_on). The runtime is the one the
/// [`TestApp`] was given and the listener is the one the server names, so the core binds nothing
/// itself: `ulo_runtime_tokio::testing::start` serves on tokio, and a smol test passes a smol
/// server to `start_on`.
///
/// Dropping it without [`close`](Self::close) aborts the serving task without running the app's
/// shutdown. The listener is released when the runtime next runs the aborted task, not inside the
/// drop; on an executor nothing runs again, as a smol `Executor` after its `block_on` returns, the
/// task is never dropped and the listener stays bound for the rest of the process. A test that
/// checks the port, or that runs on smol, ends with `close`.
pub struct TestServer {
    addresses: Vec<BoundAddr>,
    handle: AppHandle,
    task: Option<ValueHandle<Result<Shutdown, ShutdownError>>>,
}

impl TestServer {
    /// Wires and connects `app`, binds `servers` and serves them in a task of the app's runtime.
    /// `servers` is one server or a tuple, as `ulo_runtime_tokio::run` takes them. `app` carries
    /// the runtime, as `TestApp::runtime` sets it: one without a runtime is refused by `listen`,
    /// `StartupError::Bind` carrying `RuntimeMissing`.
    pub async fn start_on(app: TestApp, servers: impl Servers) -> Result<TestServer, StartupError> {
        let bound = servers.bind_to(app.connect().await?).listen().await?;
        let addresses = bound.addresses();
        let handle = bound.handle();
        let runtime = handle.runtime().expect("`listen` refuses an app with no runtime").clone();
        let task = spawn_with(&*runtime, bound.serve(std::future::pending()));
        Ok(TestServer { addresses, handle, task: Some(task) })
    }

    /// The first address bound.
    ///
    /// # Panics
    ///
    /// When no server bound one, as a broker link binds none: such a test reads
    /// [`addresses`](Self::addresses), or reaches the app through its broker.
    pub fn addr(&self) -> SocketAddr {
        self.addresses.first().expect("no server bound an address; a broker link binds none").addr
    }

    /// Every address bound, in the order the servers were given; empty when every server is a
    /// broker link.
    pub fn addresses(&self) -> &[BoundAddr] {
        &self.addresses
    }

    /// `http://<addr><path>`; `path` gets its leading `/` when it has none. The scheme is `http`
    /// whatever the server speaks: a test of another transport reads `addr`.
    pub fn url(&self, path: &str) -> String {
        let slash = if path.starts_with('/') { "" } else { "/" };
        format!("http://{}{slash}{path}", self.addr())
    }

    pub fn handle(&self) -> &AppHandle {
        &self.handle
    }

    /// Closes the app under the signal `test`, waits for the serving task to end and returns the
    /// shutdown `serve` reported. The port is released when this returns.
    ///
    /// # Panics
    ///
    /// When the serving task panicked, with the redacted message.
    pub async fn close(mut self) -> Result<Shutdown, ShutdownError> {
        let closed = self.handle.close(Signal::new("test")).await;
        let task = self.task.take().expect("the task is taken once, here");
        match task.await {
            Ok(served) => served,
            // A runtime that dropped the task has nothing left to wait for: the close's own answer
            // stands.
            Err(TaskEnd::Aborted) => closed,
            Err(end) => panic!("the serving task ended as {end:?}"),
        }
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}
