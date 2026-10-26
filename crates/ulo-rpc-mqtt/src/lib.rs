//! The MQTT v5 link for `ulo-rpc` (transports DESIGN §5.3): a topic per pattern, the payload as
//! the body with headers as user properties, replies through the Response Topic and Correlation
//! Data properties. QoS configurable; ordered per topic and QoS; the maximum packet size from
//! CONNACK; `Competing` through `$share/<group>/`, the group the application's root module's full
//! type path unless `.group(..)` names one, and a CONNACK announcing no shared subscriptions fails
//! `bind`; PUBACK or PUBREC 0x10 maps to `Unavailable` at QoS 1 and 2, and a miss at QoS 0 is the
//! client's `Timeout`; a call waiting when the client's connection drops is `Unavailable`, since a
//! clean session loses what reached the reply topic meanwhile; `mqtts://` selects TLS.
//!
//! Over `mqtts://` the link trusts the platform's root certificates, read once per link on its
//! runtime's blocking pool, a server's in `prepare` and a client's at its first connect, and kept
//! for every connection after. A store that cannot be read or holds no root is a misconfiguration
//! no retry mends: it fails a server's `bind`, and each client call `Internal` with
//! `reason: "misconfigured_link"`.
//!
//! The drain unsubscribes and returns once the broker has acknowledged each UNSUBSCRIBE, after
//! every request it routed to the instance before. Both sides announce a Receive Maximum of
//! 65,535, MQTT 5's largest, so the broker holds back no publish for its flow control until that
//! many are unacknowledged; Mosquitto otherwise applies its own `max_inflight_messages`, 20 unset.
//! A request can still be lost at shutdown only when more requests are outstanding to the instance
//! than the broker's flow control allows, the announced 65,535 or a smaller cap the broker applies
//! itself: MQTT 5 §3.10.4 lets the broker deliver a publish it held back after the UNSUBACK, and
//! one read after `close` has queued the DISCONNECT gets no answer, its caller seeing its own
//! `Timeout`. Publishes the broker queued for the instance behind a full window are dropped with
//! its session as it ends; the 65,535 Receive Maximum keeps that window as wide as MQTT 5 allows,
//! which keeps the case rare.
//!
//! A server's `close` answers every call it still holds `unavailable` with the shutdown phrase,
//! ahead of its DISCONNECT and in its first poll, and drops a reply sent after it: MQTT allows no
//! packet after the DISCONNECT. A reply finding rumqttc's request channel full waits for room, so
//! a long streamed reply is paced by the broker, and one still waiting when `close` runs is
//! dropped. An answer of `close`'s own that finds the channel full waits for room too, so it
//! reaches the caller only if the `close` is not cut first. The connection is kept for up to two
//! seconds after `close`, completed or cut, reading until the broker closes it: dropped with the
//! broker's packets unread, the socket resets the connection, and Mosquitto discards what it
//! received and had not yet read.
//!
//! The link refuses work over the server's bound rather than holding it at the broker: over
//! `Server::max_inflight`, 1,024 unset, a request is answered `unavailable` with a `RetryAfter`
//! detail, as on TCP, UDP, NATS and Redis. Holding it instead, the acknowledgement sent once the
//! handler completes and a Receive Maximum equal to the bound, needs the broker to pass an
//! unacknowledged shared-subscription message to another member when the holder's session ends,
//! which MQTT 5 §4.8.2 only recommends. Mosquitto 2.0.18 does not: in a probe of 60 runs, over a
//! clean DISCONNECT, an UNSUBSCRIBE then DISCONNECT, an aborted event loop, a killed process and a
//! keep-alive timeout, at Receive Maximum 65,535 and 4, no unacknowledged QoS 1 message reached
//! another member once its holder was gone, and held work would leave with the instance. The
//! behaviour is the broker's, not the protocol's: EMQX or HiveMQ may pass the message on, and an
//! opt-in for such a broker, declared only once the conformance drain scenario has run against it,
//! is not built.
//!
//! ```ignore
//! app.bind(ulo_rpc::Server::new(ulo_rpc_mqtt::Mqtt::url("mqtt://broker:1883")))
//! ```
//!
//! On the wire a request is the payload published on the pattern's topic with a Response Topic
//! and Correlation Data, an event the same without them. Each reply message carries one envelope
//! frame, `res`, `err`, `item` or `end`. A streamed request opens with an empty body and the user
//! property `ulo-t: open`; the server answers it on the Response Topic with `ulo-t: opened` before
//! the caller sends anything more, and the request's items, its end and every `cancel` travel on
//! `ulo/rpc/control`, which every server instance subscribes to outside the shared subscription,
//! carrying the call's Correlation Data.

mod link;

pub use link::{Mqtt, QoS};
