//! The TCP link for `ulo-rpc` on smol's sockets (transports DESIGN §5.3): `ulo-rpc-tcp`'s wire,
//! calls multiplexed by `id` over one connection, each frame a 4-byte big-endian length then the
//! frame, over `async-net`'s sockets, with `futures-rustls` for TLS. A client of either link talks
//! to a server of the other. Ordered per connection; every call shape; `Addressed`; TLS through
//! `ulo_net::Tls` with no ALPN; `goaway` on every connection at the drain.
//!
//! No tokio is involved: the sockets are `async-io`'s, whose reactor runs on a thread of its own,
//! and the link's tasks run on the runtime it runs under, `ulo_runtime_smol::Smol` on a smol app: a server's
//! app's, and a client's own, which `RpcClient` hands it. `Tcp::with_runtime` puts them on another.
//!
//! ```
//! # fn f(app: ulo::App<ulo::Connected>) {
//! app.bind(ulo_rpc::Server::new(ulo_rpc_tcp_smol::Tcp::new("0.0.0.0:7000")));
//! # }
//! ```

mod link;

pub use link::Tcp;
