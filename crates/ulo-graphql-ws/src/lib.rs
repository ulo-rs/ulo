//! GraphQL subscriptions over graphql-transport-ws (transports DESIGN §7): one hand-written
//! `ulo_ws::Gateway` over `Engine::subscribe`, for every engine.
//!
//! The protocol's `connection_init`, `ping` and `pong` carry no `id` while expecting an answer, and
//! its `complete` from the client cancels an operation, so `#[ulo_ws::message]`'s envelope does
//! not fit. The gateway owns the subprotocol check (a handshake that echoed none closes 4406),
//! `connection_init` once (a second closes 4429) with its payload kept in the session,
//! `connection_ack`, a `subscribe` before the ack closing 4401, a duplicate `id` closing 4409, an
//! unparseable message closing 4400, an init missing `connection_init_timeout` closing 4408 (1001
//! when the timer refuses the wait, having shut down), `ping` answered with `pong`, one execution
//! per `subscribe` writing `next` items and `complete` or `error`, and `complete` from the client
//! cancelling that execution with `ClientCancelled`.
//!
//! Authorization happens at connect. A connect guard runs once per connection, the operations a
//! client subscribes afterwards run past it, and no `Ws` guard, interceptor or error handler runs
//! per `subscribe`; a resolver that needs authorization decides it for itself, from what its
//! execution carries, the `connection_init` payload in [`ConnectionInit`] included.
//!
//! `ulo-graphql-http`'s `GraphqlConfig::subscriptions(path)` mounts [`GraphqlWs`] at `path`, and
//! `GraphqlConfig::connection_init_timeout` sets its timeout, bound as an [`InitTimeout`], whose
//! constructor refuses a zero bound.

mod gateway;

/// The JSON object [`ConnectionInit::payload`] answers, `serde_json`'s own types, re-exported so a
/// guard reading the payload names them with no `serde_json` dependency of its own.
pub mod json {
    #[doc(no_inline)]
    pub use serde_json::{Map, Value};
}

pub use gateway::{ConnectionInit, GraphqlWs, InitTimeout, InitTimeoutError};
