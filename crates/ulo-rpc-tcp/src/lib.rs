//! The TCP link for `ulo-rpc` (transports DESIGN §5.3): calls multiplexed by `id` over one
//! connection, each frame a 4-byte big-endian length then the frame, bounded by `max_frame`: a
//! server answers a larger frame `bad_request` with `payload_too_large` where its first bytes name
//! its call, then reads the frame's declared length and drops it, the body never buffered, and the
//! connection carries the frames after it. Ordered per
//! connection; every call shape; `Addressed`, an event reaching the server its caller names, so an event declared
//! `every_instance` is refused at startup; TLS through `ulo_net::Tls` with no ALPN; `goaway` on every connection
//! at the drain.
//!
//! The crate is tokio-bound: the link spawns through `ulo-runtime-tokio`, so it is built inside a tokio
//! runtime or given one with `with_handle`. `ulo-rpc-tcp-smol` carries the same wire on smol.
//!
//! ```
//! # fn f(app: ulo::App<ulo::Connected>) {
//! app.bind(ulo_rpc::Server::new(ulo_rpc_tcp::Tcp::new("0.0.0.0:7000")));
//! # }
//! ```

mod link;

pub use link::Tcp;
