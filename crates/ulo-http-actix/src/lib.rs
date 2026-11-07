//! Runs a `ulo` application inside an actix-web server (transports DESIGN §3.8): a native
//! embedding, the adapter implementing `HttpServiceFactory`, as a scope or the default service.
//! The app's pipeline runs inside actix's single-threaded workers, so actix's per-core runtime
//! model is kept.
//!
//! ```
//! # use ulo::App;
//! # #[ulo::module]
//! # struct AppModule;
//! # async fn f() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
//! let server = ulo_http_actix::Embedded::new().nested_at("/api");
//! let embedded = server.handle();
//! let app = App::builder(AppModule).runtime(ulo_runtime_tokio::Tokio::current()).wire()?.connect().await?.bind(server).listen().await?;
//!
//! let host = {
//!     let embedded = embedded.clone();
//!     actix_web::HttpServer::new(move || actix_web::App::new().service(ulo_http_actix::scope("/api", &embedded)))
//!         .bind(("0.0.0.0", 8080))?
//! };
//! ulo_http_actix::run(app, &embedded, host, ulo_runtime_tokio::shutdown_signal()).await?;
//! # Ok(())
//! # }
//! ```
//!
//! actix's request payload is `!Send` while the app's body is `Send`, so the payload is pumped
//! through a bounded channel from the worker-local task. `web::scope` leaves the path whole, so the
//! adapter is declared without `strips_prefix` and the app strips `.nested_at` itself. A
//! `Connection: close` header is mapped onto the response head's connection-type flag. What it
//! declares: the peer address from `peer_addr`; no upgrades; no miss forwarding; no host
//! extensions, a value in actix's request store crossing through a `forward` copy over
//! `HttpRequest`; no TLS info; streamed bodies through the pump; a dropped body observed at the
//! next failed write; a request still arriving at the drain closed by the host; an HTTP/2
//! connection kept to the stop deadline without GOAWAY.
//!
//! The request still arriving is `DrainPending::Closed`. From actix-web 4.15 actix's graceful stop reaches every
//! HTTP/1 connection, and a connection with no complete request in progress closes at once. A
//! client caught mid-request at the moment of shutdown, its head not yet fully sent, gets a closed
//! connection rather than the app's 503, and a request pipelined behind one in flight is dropped
//! the same way.
//!
//! HTTP/2 is `DrainHttp2::Reset`. The adapter builds actix-web without its `http2` feature, so on
//! its own it serves HTTP/1.1 alone, but Cargo unifies features: any crate in the application
//! that enables actix-web's `http2` turns it on here too, and a server listening with
//! `listen_auto_h2c` or over TLS with ALPN then serves HTTP/2. actix's graceful stop sends such a
//! connection no GOAWAY. It stays open, its requests reaching the draining app and answered 503,
//! until `shutdown_timeout`, which `run` sets to the drain window, and is then reset; while one is
//! open, the app's `close` takes the whole window.
//!
//! The `conformance-http2` feature is for the conformance suite; it enables actix-web's `http2`
//! and changes nothing in the adapter. An application serving HTTP/2 enables actix-web's `http2`
//! itself.
//!
//! Built-in forwards: `OriginalPath`, from `req.path()`. actix's response extensions are its own
//! store, so of the app's response extensions only `Routing` is copied into them, where an actix
//! middleware reads it.

mod pump;
mod run;
mod service;

use ulo_http::embed::{Disconnect, DrainHttp2, DrainPending, Embed, EmbedLimits, OriginalPath};

pub use run::run;
pub use service::{ActixScope, ActixService, scope};

/// The actix-web host.
pub struct Actix;

impl Embed for Actix {
    const NAME: &'static str = "actix";

    type HostRequest<'r> = actix_web::HttpRequest;

    fn limits() -> EmbedLimits {
        EmbedLimits::NONE
            .without_upgrades("actix's upgrade is a payload and body pair on the request, not a per-request future handing over the upgraded I/O")
            .without_forward_miss("an actix scope or default service answers every request it is handed, and offers no way to hand one back to the router")
            .without_host_extensions(
                "actix's request extensions are its own store, which cannot be enumerated; a value in it crosses through an `Embedded::forward` copy",
            )
            .without_tls_info("the adapter reads no TLS state off actix's request, so `ConnInfo::tls` is `None`")
            .without_strips_prefix("`web::scope` leaves `req.path()` whole, with the rest in `match_info`")
            .without_pending_close("actix-http keeps running a call whose connection the peer closed until the call answers")
            .without_pending_reset("actix-http keeps running a call whose HTTP/2 stream the peer reset until the call answers")
            .disconnect(Disconnect::AtNextWrite)
            .drain_pending(DrainPending::Closed)
            .drain_http2(DrainHttp2::Reset)
    }

    fn builtin_forwards(embedded: ulo_http::embed::Embedded<Self>) -> ulo_http::embed::Embedded<Self> {
        embedded.forward(|req: &actix_web::HttpRequest| Some(OriginalPath::new(req.path())))
    }
}

/// The embedding server for actix-web, `ulo_http::embed::Embedded<Actix>`.
pub type Embedded = ulo_http::embed::Embedded<Actix>;

/// Its handle, `ulo_http::embed::Handle<Actix>`.
pub type Handle = ulo_http::embed::Handle<Actix>;
