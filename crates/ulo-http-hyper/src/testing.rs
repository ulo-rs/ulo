//! A test serves its app over a real socket: [`TestServer`], the core's
//! `ulo::testing::TestServer`, and with the `tokio` feature [`TestServerExt::start`], which binds
//! this backend on port 0 over tokio's sockets.
//!
//! The type is the core's because serving binds a socket through whatever `ulo::Server` is given and
//! names no HTTP crate: a gRPC-only or WebSocket-only app serves with `start_on` and
//! `ulo_runtime_tokio::testing::start` and does not depend on this one. `start_on` takes any server,
//! `ServerOn::<SmolListener>::new("127.0.0.1:0")` on smol included.
//!
//! ```
//! # use ulo::testing::TestApp;
//! # use ulo_http_hyper::testing::{TestServer, TestServerExt};
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

pub use ulo::testing::TestServer;

/// Adds [`start`](Self::start) to [`TestServer`]: the core's type cannot name this backend.
#[cfg(feature = "tokio")]
pub trait TestServerExt {
    /// Wires and connects `app`, binds an HTTP server on `127.0.0.1:0` over tokio's sockets and
    /// serves it. `app` carries the runtime: `TestApp::of(AppModule).runtime(Tokio::current())`.
    fn start(app: ulo::testing::TestApp) -> impl Future<Output = Result<TestServer, ulo::StartupError>>;
}

#[cfg(feature = "tokio")]
impl TestServerExt for TestServer {
    fn start(app: ulo::testing::TestApp) -> impl Future<Output = Result<TestServer, ulo::StartupError>> {
        TestServer::start_on(app, crate::Server::new("127.0.0.1:0"))
    }
}
