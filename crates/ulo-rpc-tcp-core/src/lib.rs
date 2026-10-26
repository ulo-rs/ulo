//! The runtime-free half of `ulo-rpc`'s TCP links (transports DESIGN §5.3), shared by `ulo-rpc-tcp`
//! on tokio and `ulo-rpc-tcp-smol` on `async-io`'s sockets.
//!
//! An implementation crate for link authors: a TCP link over another runtime's sockets is built on
//! it, as those two are. Application code does not depend on it; an application names
//! `ulo_rpc_tcp::Tcp` or `ulo_rpc_tcp_smol::Tcp`, which carry this wire. Its names and shapes follow
//! what the links need and change with them.
//!
//! What lives here is everything the wire decides: each frame a 4-byte big-endian length then the
//! frame, bounded by `max_frame`; a server mapping each connection's call ids to ids of its own;
//! the writer queue each connection's frames go out through in the order their sends were called;
//! `goaway` at the drain, a connection registered after it included; the inbound stream ending
//! once the draining server holds no call, the link answering later requests `unavailable`; each
//! server connection lingering after `close` until its peer closes its half, for at most
//! [`CUT_GRACE`] on the app's `Timer`; and a client's connections ending at `close`. A link
//! supplies its sockets: a listener adopted from `ulo-net`'s
//! [`BoundListener`](ulo_net::BoundListener) behind [`Accept`], each accepted connection's TLS
//! handshake in an [`Incoming`], and a connected client's two halves, all on `futures-io`'s
//! traits. Every task the core starts is spawned on the runtime the link hands it, and every wait
//! goes through the app's `Timer`, that runtime's timer, or a channel that needs none.

mod link;

pub use link::{Accept, CUT_GRACE, Core, DEFAULT_MAX_FRAME, Incoming, Reader, Writer};
