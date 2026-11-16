//! The TCP link for `ulo-rpc` (transports DESIGN §5.3): calls multiplexed by `id` over one
//! connection, each frame a 4-byte big-endian length then the frame, bounded by `max_frame`: a
//! server answers a larger frame `bad_request` with `payload_too_large` where its first bytes name
//! its call, and closes the connection either way, the body never buffered. Ordered per
//! connection; every call shape; `Addressed`, an event reaching the server its caller names, so an event declared
//! `every_instance` is refused at startup; TLS through `ulo_net::Tls` with no ALPN; `goaway` on every connection
//! at the drain.
//!
//! ```
//! # fn f(app: ulo::App<ulo::Connected>) {
//! app.bind(ulo_rpc::Server::new(ulo_rpc_tcp::Tcp::new("0.0.0.0:7000")));
//! # }
//! ```

mod link;

pub use link::Tcp;
