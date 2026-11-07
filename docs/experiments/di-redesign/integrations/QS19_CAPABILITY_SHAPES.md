# QS-19: the capability shapes

Paths are relative to the repository root; `path:N` is a line in the tree this document sits in.

## The shapes

Where a host or link says what it can do, read at `prepare` or by a conformance suite, there is one declaration: a struct of named capabilities (S1). The other mechanisms answer questions a declaration cannot, and each keeps its own shape (S5 to S7, and three members of S4).

| # | Shape | Used in | Declared by | Read by | Absent | Reason carried | Default |
|---|---|---|---|---|---|---|---|
| S1 | Struct, every boolean capability a private field, `without_<capability>(reason)` the one way to declare it absent | RPC `Capabilities` `crates/ulo-rpc/src/link.rs:310`; HTTP `BackendLimits` `crates/ulo-http/src/backend.rs:74`; HTTP `EmbedLimits` `crates/ulo-http/src/embed.rs:123`; the database suite's `Capabilities` `crates/ulo-db-conformance/src/lib.rs:95` | the link from `capabilities(&self)`, the backend from `limits(&self)`, the `Embed` adapter from `limits()`, a database suite host from `Host::capabilities(&self)` | `Server::prepare` (`crates/ulo-rpc/src/server.rs:119`, `crates/ulo-http/src/server.rs:415`, `crates/ulo-http/src/embed.rs:552`), `Service::respond` (`embed.rs:1109`), the client, the RPC, HTTP, WebSocket and database suites | a refusal or `warn` at `prepare` naming the capability and the reason; in a suite, the branch the declaration selects, or a skip that writes the reason | yes, mandatory, `&'static str`, read through `why_not_<capability>()` | every boolean held: `Capabilities::new`, `BackendLimits::NONE`, `EmbedLimits::NONE`, the database suite's `Capabilities::new` and `default` |
| S4 | Trait method with a default body, or one returning `Option` | `Embed::take_upgrade` `embed.rs:107`; `Spawn::name` `crates/ulo/src/runtime.rs:45`; `Engine::playground_html` `crates/ulo-graphql/src/lib.rs:37`; `Harness::stoppable` `crates/ulo-runtime-conformance/src/lib.rs:55` | override the method | caller of the method | the default body | no | the body |
| S5 | Required predicate method plus a reason method with a default | `Store::create_connects(o)`, `Store::connects_because(o)` `crates/ulo-db/src/store.rs:29,34` | implement both for the store | `StoreModule::register` | the predicate has no default; the reason defaults to a generic text | yes, per options value | none for the predicate |
| S6 | Optional sub-trait gating a method by a method bound | `MigrationLock: Store` `crates/ulo-db/src/migrate.rs:127`, gating `StoreModule::migrate` `crates/ulo-db/src/module.rs:173` | `impl MigrationLock for Postgres` | the compiler, at the call | E0277 carrying the trait's `#[diagnostic::on_unimplemented]` message, and a note naming the stores that implement it | in the diagnostic | absent |
| S7 | Refusal returned by a probing call | `Tokio::require_io` `crates/ulo-runtime-tokio/src/lib.rs:91`; `Timer::try_sleep` and `Refused` `crates/ulo/src/timer.rs:36,42`; `NoTimer` `link.rs:589`; `UnsupportedRuntime` `crates/ulo-db/src/connect.rs` | nothing declared; the environment is probed | the link or module at `usable`/`prepare`/`listen`/`connect` | `Err` with a typed error whose `Display` names the cause | yes, in the error text | none |
| S8 | Absence declared in a suite, with a reason, for what no capability describes and no refusal carries | `conformance_suite!(.. not_applicable { name: "reason" })` in `ulo-rpc-conformance`, `ulo-http-conformance`, `ulo-ws-conformance`, `ulo-runtime-conformance`, `ulo-db-conformance` | the link's or host's `tests/conformance.rs` | the macro, which stamps `#[ignore = "not applicable: .."]` | the test is ignored, and fails if run | yes, a required string literal | none |

S2 and S3 are not shapes of their own: the reasoned setter is S1's rule for every capability, `single_delivery` among them, and whether a host strips the mount prefix is `EmbedLimits::strips_prefix`. `Transport::READS_PREFIX` and `BUILDS_CONTROLLER` (`crates/ulo/src/transport/mod.rs:47,55`) stay associated consts: they describe a transport, not a host or a link. `Link::NAME`, `Backend::NAME` and `Store::NAME` are identities.

An app-side builder knob sits beside S1 on HTTP: `Embedded::peer_addr(true)` overrides a host declared without `peer_addr`. Link constructors carry others (`.codec(..)`, `.max_frame(..)`, `.with_handle(..)`); they set configuration that the link then reports through S1's public fields, which are reports of configuration and no abilities: RPC `binary`, `max_frame`, `ordering`, `shapes` and `delivery`.

## S1, the declaration

Declaration (RPC), `crates/ulo-rpc-udp/src/link.rs:197`:

```rust
fn capabilities(&self) -> Capabilities {
    Capabilities::new(DeliveryMode::Addressed)
        .codec(self.codec)
        .max_frame(Some(MAX_DATAGRAM))
        .ordering(ulo_rpc::Ordering::Unordered)
        .shapes(UNARY_ONLY)
        .without_native_backpressure("a UDP server reads each request off its socket as it arrives, ...")
        .without_unserved_hold("the caller sends to one server's address, ...")
        .without_durable_replies("a reply is one datagram sent once to the caller's address, ...")
        .without_confirmed_drain("`close` answers the requests its socket already holds, but a datagram arriving after the socket closes reaches no one; ...")
        .without_ordered_control("datagrams arrive in any order, so a `cancel` may overtake its request")
}
```

Declaration (HTTP), `crates/ulo-http-actix/src/lib.rs:74`:

```rust
fn limits() -> EmbedLimits {
    EmbedLimits::NONE
        .without_upgrades("actix's upgrade is a payload and body pair on the request, not a per-request future handing over the upgraded I/O")
        .without_forward_miss("an actix scope or default service answers every request it is handed, ...")
        .without_host_extensions("actix's request extensions are its own store, which cannot be enumerated; ...")
        .without_tls_info("the adapter reads no TLS state off actix's request, so `ConnInfo::tls` is `None`")
        .without_strips_prefix("`web::scope` leaves `req.path()` whole, with the rest in `match_info`")
        .disconnect(Disconnect::AtNextWrite)
        .drain_pending(DrainPending::Closed)
        .drain_http2(DrainHttp2::Reset)
}
```

Read, `crates/ulo-rpc/src/server.rs:205` and `crates/ulo-http/src/embed.rs:552`:

```rust
if let Some(reason) = capabilities.why_not_single_delivery()
    && requests > 0
{
    tracing::warn!(link = L::NAME, request_patterns = requests, reason, "the {} link delivers each request to every server instance ...")
```
```rust
if let Some(reason) = limits.why_not_upgrades()
    && upgrading
{
    ... "the {host} embedding is declared without `upgrades` ({reason}), so a gateway on the HTTP transport cannot take an upgrade ..."
```

Properties:

- Every boolean capability is held unless declared absent, on RPC as on HTTP and in the database suite, the strict reading of `DESIGNER_HANDOFF.md` §3.9, which is for abilities a suite checks: a link or host that declares nothing is held to each, and a scenario fails where it cannot keep one. On RPC that covers `native_backpressure`, `miss_signal`, `unserved_hold`, `durable_replies`, `confirmed_drain`, `ordered_control` and `single_delivery`; on `BackendLimits` `upgrades`, `h2c`, `inherited_sockets`, `port_zero` and `tls`; on `EmbedLimits` `peer_addr`, `upgrades`, `forward_miss`, `host_extensions`, `tls_info` and `strips_prefix`; on the database suite's `Capabilities` `lazy` and `migrations`.
- A `false` always carries its reason. The fields are private, a `compile_fail,E0616` doctest on each `Capabilities` pins that, `without_<capability>(reason)` is the one setter for each, `<capability>()` answers whether it is held under the positive name, and `why_not_<capability>()` answers the reason.
- What reports configuration rather than an ability stays a public field with a `const fn` setter each and a default of its own: RPC `binary`, `max_frame`, `ordering`, `shapes`, `delivery`; `EmbedLimits`' `request_body`, `disconnect` and the three drain enums, whose variants refuse nothing. RPC `binary` is `false` from `new`, a JSON link, since it changes what the link speaks on the wire, and `Capabilities::codec(codec)` sets it from the link's codec, `true` for `Codec::Cbor`. A binary payload on a JSON link is refused naming `.codec(Codec::Cbor)`.
- A link's and a backend's declaration is built through `&self`, so it can follow configuration: the codec decides `binary`, MQTT's QoS `miss_signal`, a NATS or MQTT link's `max_frame` what the server announced. An `Embed` adapter is a type naming its host and has no value to read, so `Embed::limits()` stays an associated function.
- A suite scenario that needs a capability to run at all skips from the declaration, writing ``skipped `<scenario>`: declared without `<capability>`: <reason>`` to standard error, which libtest does not capture: the RPC suite's `cancel_follows_its_request` from `ordered_control` (`crates/ulo-rpc-conformance/src/cases/order.rs:163`), the database suite's lazy scenario from `lazy` and its two migration scenarios from `migrations` (`crates/ulo-db-conformance/src/cases/lazy.rs:23`, `migrate.rs:11`), and every WebSocket scenario but the declaration's from `Host::why_not_upgrades()` (`crates/ulo-ws-conformance/src/lib.rs:124`), which an embed host reads off its adapter's `EmbedLimits`. A scenario whose capability selects between two answers, `miss_signal`'s `Unavailable` or `Timeout` and the like, runs both ways and prints the reason in the failure of the absent branch. A scenario a link's `shapes` exclude asserts the startup refusal they declare: every streamed scenario on UDP, `cancel_before_opened` and the three held-cancel scenarios among them. libtest counts a skip as a pass, so `crates/ulo-rpc-conformance/tests/skips.rs` holds the RPC gate in both directions with a link that fails whatever starts it, and the database suite's `tests/create_loop.rs` runs its lazy and migration scenarios on a host declared without both, where a scenario ignoring the declaration fails at the start it refuses.

## What each shape answers that S1 cannot

- A reason that depends on the options value (S5): `connects_because` takes the options; an S1 reason is one `&'static str` per declaration, chosen when the declaration is built.
- A capability with its own types and methods (S6): a bool cannot carry `MigrationLock::Guard`. Its absence is a compile error, and `StoreModule::migrate` carries the bound on the method, `where S: MigrationLock`, so the error is E0277 and prints the trait's `on_unimplemented` text; a bound on the impl block would hide the method as E0599, which prints none. `crates/ulo-codegen-tests/tests/ui/migrate_without_a_migration_lock.rs` holds the diagnostic.
- A change over time (S7): a runtime that shuts down, or an IO driver that was never enabled.
- An optional behaviour or an identity (S4): `take_upgrade` is consulted only on a host that has `upgrades`; `Spawn::name` names a runtime; `playground_html` is an optional feature no suite claims.
- A scenario no capability describes and no refusal carries (S8): `not_applicable` keeps its required reason, for a limit of the environment rather than of the link, UDP's recovery scenario, rocket's drain scenario, and the held-cancel scenarios on a link that carries streamed requests and whose client holds no `cancel`, TCP's, Redis's and the community fixture's.

## Open beside the decision

- A reader generic over `Capabilities`, `BackendLimits` and `EmbedLimits`, listing `(name, held, reason)`, waits for a consumer such as a generated docs table.
- The held-cancel scenarios stay `not_applicable` on TCP, Redis and the community fixture, which carry streamed requests and hold no `cancel`: whether a client holds one is `Broker::probe` returning `None` (S4), not a declared capability, and nothing is refused there to assert instead.
