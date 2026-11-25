//! A test serves its app over a real socket: [`TestServer`] binds an HTTP server on port 0,
//! serves in a task of the app's runtime and closes through the app's shutdown.
//!
//! It lives here, not in `ulo::testing`, because it binds a socket and the core is runtime-free.
//! The runtime is the one the `TestApp` was given; the listener is the one the server names, so
//! `start` serves on tokio's sockets and `start_on` takes any other server, smol's included.
//!
//! ```
//! # use ulo::testing::TestApp;
//! # use ulo_http_hyper::testing::TestServer;
//! # use ulo_runtime_tokio::Tokio;
//! # #[ulo::module]
//! # struct AppModule;
//! # async fn f() -> Result<(), Box<dyn std::error::Error>> {
//! let served = TestServer::start(TestApp::of(AppModule).runtime(Tokio::current())).await?;
//! let url = served.url("/notes/1");
//! // ... send a request to `url` with any HTTP client ...
//! served.close().await?;
//! # Ok(())
//! # }
//! ```

use std::net::SocketAddr;

use ulo::testing::TestApp;
use ulo::{AppHandle, Shutdown, ShutdownError, Signal, StartupError, TaskEnd, ValueHandle, spawn_with};

/// An app bound and serving. Dropping it without [`close`](Self::close) aborts the serving task
/// without running the app's shutdown. The listener is released when the runtime next runs the
/// aborted task, not inside the drop; on an executor nothing runs again, as a smol `Executor` after
/// its `block_on` returns, the task is never dropped and the listener stays bound for the rest of
/// the process. A test that checks the port, or that runs on smol, ends with `close`.
pub struct TestServer {
    addr: SocketAddr,
    handle: AppHandle,
    task: Option<ValueHandle<Result<Shutdown, ShutdownError>>>,
}

impl TestServer {
    /// Wires and connects `app`, binds an HTTP server on `127.0.0.1:0` over tokio's sockets and
    /// serves it. `app` carries the runtime: `TestApp::of(AppModule).runtime(Tokio::current())`.
    #[cfg(feature = "tokio")]
    pub async fn start(app: TestApp) -> Result<TestServer, StartupError> {
        Self::start_on(app, crate::Server::new("127.0.0.1:0")).await
    }

    /// As [`start`](Self::start) with `servers` in place of the HTTP server on tokio: one server or
    /// a tuple, as `ulo_runtime_tokio::run` takes them, such as
    /// `ServerOn::<SmolListener>::new("127.0.0.1:0")` on smol. `addr` is the first address bound.
    pub async fn start_on(app: TestApp, servers: impl ulo::Servers) -> Result<TestServer, StartupError> {
        let bound = servers.bind_to(app.connect().await?).listen().await?;
        let addr = bound.addresses().first().expect("the app bound an address").addr;
        let handle = bound.handle();
        let runtime = handle.runtime().expect("a bound app has a runtime").clone();
        let task = spawn_with(&*runtime, bound.serve(std::future::pending()));
        Ok(TestServer { addr, handle, task: Some(task) })
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// `http://<addr><path>`; `path` gets its leading `/` when it has none.
    pub fn url(&self, path: &str) -> String {
        let slash = if path.starts_with('/') { "" } else { "/" };
        format!("http://{}{slash}{path}", self.addr)
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
