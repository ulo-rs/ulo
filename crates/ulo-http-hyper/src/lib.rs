//! The hyper backend for `ulo-http` (transports DESIGN §3.7), the reference server and the
//! default: one `ulo_http::Backend` that owns the listeners. `ulo-server-hyper`'s accept loop
//! accepts and performs the TLS handshake; each connection is served by hyper's connection
//! builders, configured from the server's settings, and each request is converted into a
//! `ulo_http::Request` and answered by the `AppService`. `ulo-http` owns routing.
//!
//! ```
//! # use ulo::{App, Connected};
//! # async fn f(app: App<Connected>) -> Result<(), Box<dyn std::error::Error>> {
//! let app = app.bind(ulo_http_hyper::Server::new("0.0.0.0:8080")).listen().await?;
//! # Ok(())
//! # }
//! ```
//!
//! The runtime is the listener's: [`Server`] serves on tokio's sockets through `ulo-listener-tokio`,
//! the default `tokio` feature, and [`ServerOn`] on any other listener, smol's through
//! `ulo-listener-smol`:
//!
//! ```
//! # use ulo::{App, Connected};
//! # fn f(app: App<Connected>) {
//! let server = ulo_http_hyper::ServerOn::<ulo_listener_smol::SmolListener>::new("0.0.0.0:8080");
//! app.bind(server);
//! # }
//! ```
//!
//! Either way the connections, HTTP/2's stream tasks and the header-read clock run on the app's
//! runtime and `Timer`.
//!
//! Documented limits: none. HTTP/1.1 and HTTP/2 (h2c when the server enables it), TLS through
//! `ulo_net::Tls`, inherited sockets, port 0, and per-request upgrades through hyper's upgrade
//! future.
//!
//! `header_timeout` is hyper's HTTP/1.1 header-read timeout; an HTTP/2 connection has no
//! head-read clock. `max_concurrent_streams` at `Count::Default` sends hyper's own value, which
//! hyper places outside its stability guarantee.
//!
//! An application running inside another framework's server binds `ulo_http::embed::Embedded`
//! through that framework's adapter crate instead.

mod backend;
mod convert;

pub use backend::HyperOn;
pub use ulo_server_hyper::ReadCount;

/// The hyper backend on tokio's sockets, `ulo-listener-tokio`'s listener: the default.
#[cfg(feature = "tokio")]
pub type Hyper = HyperOn<ulo_listener_tokio::TokioListener>;

/// `ulo_http::Server` over hyper on tokio: `ulo_http_hyper::Server::new("0.0.0.0:8080")`.
#[cfg(feature = "tokio")]
pub type Server = ulo_http::Server<Hyper>;

/// `ulo_http::Server` over hyper on listener `L`, the runtime's sockets:
/// `ulo_http_hyper::ServerOn::<ulo_listener_smol::SmolListener>::new("0.0.0.0:8080")` on smol.
pub type ServerOn<L> = ulo_http::Server<HyperOn<L>>;
