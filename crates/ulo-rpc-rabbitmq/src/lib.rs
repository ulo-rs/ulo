//! The RabbitMQ link for `ulo-rpc`, AMQP 0-9-1 (transports DESIGN §5.3): a queue per pattern, a
//! request reaching it through the default exchange and an event through the pattern's fanout
//! exchange `ulo.rpc.event.<pattern>`, which an event declared `every_instance` also copies to an
//! exclusive auto-delete queue of each instance's own, the payload as the body with AMQP headers, replies through
//! `reply_to` and `correlation_id`. At-least-once with the ack after the handler completes, so
//! handlers must be idempotent; ordered per queue with a single consumer; `Competing`; one
//! prefetch window for every pattern following the server's `max_inflight`, 1,024 unset and 64
//! under `Count::Unlimited`, so a request over the bound waits in its queue; the
//! client channel in confirm mode with `mandatory` publishes, so `basic.return` maps to
//! `Unavailable`; an unhandled event is `basic.reject`ed without requeue; a call waiting when the
//! client's connection drops is `Unavailable`, since a direct reply-to address dies with its
//! channel; the drain cancels the consumers (`basic.cancel`); `amqps://` selects TLS. A server's
//! `listen` and a client's connect wait for the broker at most [`RabbitMq::connect_within`], 30 s
//! unset, and a wait that runs out fails with [`ConnectTimedOut`]; a client runs one connect at a
//! time, which every call waits on and none cancels by giving up. A server's `listen` needs the
//! app's `Timer`. A server's `close` answers every call it still holds `Unavailable` with the
//! shutdown phrase and closes its connection once its writer has published those answers and
//! every reply queued before them, from a task the close does not hold, so a close cut short by
//! a spent shutdown cap still delivers them; the task lets go of the connection at the latest 2 s
//! after the close began, on the app's `Timer`.
//!
//! The crate is tokio-bound: the link spawns through `ulo-runtime-tokio`, so it is built inside a tokio
//! runtime or given one with `with_handle`.
//!
//! ```
//! # fn f(app: ulo::App<ulo::Connected>) {
//! app.bind(ulo_rpc::Server::new(ulo_rpc_rabbitmq::RabbitMq::url("amqp://guest:guest@mq:5672/%2f")));
//! # }
//! ```
//!
//! On the wire a request is the payload published to the pattern's queue with `reply_to` and
//! `correlation_id`, and an event the payload without them, published to the pattern's event
//! exchange with the pattern as its routing key. The client receives replies through
//! RabbitMQ's direct reply-to, each message one envelope frame, `res`, `err`, `item` or `end`. A
//! streamed request opens with an empty body and the header `ulo-t: open`; the server answers it
//! with `ulo-t: opened` before the caller sends anything more, and the request's items, its end and
//! every `cancel` travel through the fanout exchange `ulo.rpc.control`, which binds one queue per
//! server instance, carrying the call's `correlation_id`. A queue outlives the server that
//! declared it, so a request sent while no instance consumes waits in the queue, and a caller sees
//! its own `Timeout` rather than `Unavailable`: the link declares `unserved_hold`.
//!
//! AMQP caps an exchange's name, a queue's, a routing key and a header's name at 255 bytes, so
//! the link serves a pattern of at most 241 bytes, whose event exchange `ulo.rpc.event.<pattern>`
//! is then 255, and an event declared `every_instance` on one of at most 208, whose queue of the
//! instance's own, `ulo.rpc.event.<pattern>.<uuid>`, is then 255. A server refuses a longer
//! pattern, and a longer one whose event is declared `every_instance`, in `prepare`, through
//! `Link::subscribable`: a permanent `StartupError::Configure` entry naming each such pattern, its
//! length in bytes and the limit, reported with every other server's configuration failures
//! before any server binds. A client fails an event whose exchange name, a request or a stream
//! whose routing key, or a call carrying a header whose name, would be longer, before publishing
//! it, as `ulo_rpc::NameTooLong`, which the caller reads as `BadRequest` with the reason
//! `name_too_long`, and the connection carries the frames after it.
//!
//! Upgrade the servers before the clients. A client publishes each event to the pattern's
//! exchange, which a server of `ulo-rpc-rabbitmq` 0.1.0-alpha.2 or earlier binds no queue to:
//! until a server of this release has declared the pattern, the client's events are dropped at the
//! broker with no error at the client. A client of 0.1.0-alpha.2 or earlier publishes events to the
//! default exchange, which reaches a pattern's shared queue and no queue of an instance's own, so
//! its events reach no handler declared `every_instance`.

mod link;

pub use link::RabbitMq;
pub use ulo_rpc::ConnectTimedOut;
