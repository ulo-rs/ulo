//! The NATS link for `ulo-rpc` (transports DESIGN §5.3): the payload as the message body with
//! NATS headers, replies on the `_INBOX` subject, stream items on the inbox. At-most-once;
//! ordered per publisher and subject; the maximum payload read from the server's `INFO`;
//! `Competing` through a queue group, the application's root module's full type path unless
//! `.group(..)` names one; no-responders maps to `Unavailable`, and so does a call waiting when
//! the client's connection drops, since async-nats resubscribes the inbox but what was published
//! to it meanwhile is gone; the drain is NATS's drain protocol, which async-nats ends without
//! waiting for the server to stop routing, so a request routed to the draining instance in between
//! can be lost and its caller sees its own `Timeout` (`confirms_drain` is `false`); `tls://` selects
//! TLS, and a `nats://` server requiring it upgrades the connection, either trusting the platform's
//! root certificates. The process reads them once while any link holds them and writes them to one
//! PEM file in the OS temp directory, removed once the last link and connection holding it have
//! dropped it; a process killed first leaves that one file, uniquely named and holding public
//! data. A `tls://` link waits for the load. A `nats://` link does not: a connection made before
//! the load ends gets no file, so if its server upgrades it, async-nats reads the platform's store
//! on the thread polling it. A `nats://` link whose load fails connects without the file.
//!
//! ```ignore
//! app.bind(ulo_rpc::Server::new(ulo_rpc_nats::Nats::url("nats://bus:4222")))
//! ```
//!
//! On the wire a request is the payload published on the pattern's subject with a reply subject,
//! an event the same without one, so `nats req invoices.create '{..}'` reaches a handler. Each
//! reply message carries one envelope frame, `res`, `err`, `item` or `end`. A streamed request
//! opens with an empty body and the header `ulo-t: open`; the server answers it on the reply
//! subject with `ulo-t: opened` before the caller sends anything more, and the request's items,
//! its end and every `cancel` travel on `ulo.rpc.control`, which every server instance reads, with
//! the call's reply subject as their reply subject.

mod link;

pub use link::Nats;
