# QS-19: the capability shapes side by side

Read against `toni-rs` master `10f972e6`. Paths are relative to the repository root; `path:N` is a line in that tree. A claim marked "unverified" was not checked against source.

## The choice

S10 (`docs/experiments/di-redesign/salvage/build-sequence.md:123`) asked for one capability declaration across HTTP, WebSocket, RPC and the GraphQL engine SPI. HEAD has none shared: each SPI declares in its own way, and the way differs by mechanism, not only by crate. Eight mechanisms are in use (S1 to S8 below). `Capabilities::fans_out_requests(reason)` (Q22-P7) is S2, the one that makes a `false` carry its reason. The question for the designer is whether to keep per-SPI declarations as they are, to converge on one mechanism for "a link or host declares what it can do and core reads it at `prepare`" (S1, optionally with S2's reason rule), or to leave the mechanisms as they are and record that the choice was made. `build-sequence.md:446-448` and `C-X17` (`:169`) record the earlier recommendation to accept per-transport shapes "unless a consumer reads capabilities generically; nothing at HEAD does". The reads listed in the table go through each struct directly; no common trait or type was found in the files read (a workspace-wide search for a generic reader was not run: unverified).

How the ledger's count maps onto this list is unverified. The S10 row names four SPIs; Q22-P7 calls `fans_out_requests` "a fifth capability shape"; QS-19 says "four declaration shapes"; none of the three enumerates the shapes. The list below is cut by mechanism.

Not covered because nothing there declares a capability: `crates/ulo-grpc` (a search for `capabilit`, `Limits`, `not_applicable` finds none; its `Shape` matches in `server.rs:310` describe call shapes) and `crates/ulo-ws` beyond the test-suite `Host::upgrades` (S8) and the `Transport` consts (S3).

## Table

| # | Shape | Used in | Declared by | Read by | Absent or false | Reason carried | Default |
|---|---|---|---|---|---|---|---|
| S1 | Struct, public fields, `#[non_exhaustive]`, `const fn` setters | RPC `Capabilities` `crates/ulo-rpc/src/link.rs:272-321`; HTTP `BackendLimits` `crates/ulo-http/src/backend.rs:62-96`; HTTP `EmbedLimits` `crates/ulo-http/src/embed.rs:129-196` | the link or host returns it from `capabilities(&self)` / `limits()` | `Server::prepare` (`crates/ulo-rpc/src/server.rs:169,180`), HTTP `prepare` (`server.rs:413-428`, `embed.rs:488-520`), the client, the conformance suites | a refusal at `prepare` naming the limit (HTTP; RPC shapes and binary), or a conformance test that fails or skips (RPC flags) | no | RPC: `Capabilities::new(delivery)` strict per field (`link.rs:334`); HTTP: `BackendLimits::NONE`, `EmbedLimits::NONE` all true |
| S2 | Struct with private fields and one setter taking a reason | RPC `Capabilities::fans_out_requests` `link.rs:405`, accessors `:428,:434` | `Capabilities::new(..).fans_out_requests(REASON)` | `Server::prepare` `server.rs:205`; suite `cases/delivery.rs:79` | `delivers_requests_once()` is `true`, `fan_out_reason()` is `None` | yes, mandatory, `&'static str` | `true` |
| S3 | Trait associated const with a default | `Embed::STRIPS_PREFIX` `embed.rs:94`; `Transport::READS_PREFIX`, `BUILDS_CONTROLLER` `crates/ulo/src/transport/mod.rs:47,55`; `Link::NAME`, `Backend::NAME`, `Store::NAME` (identity, not capability) | `const X: bool = ..;` in the impl | core, as `A::STRIPS_PREFIX` `embed.rs:1036`, `T::READS_PREFIX` `controller.rs:53` | the default applies | no, a doc comment only | per const |
| S4 | Trait method with a default body, or one returning `Option` | `Embed::take_upgrade` `embed.rs:112`; `Spawn::name` `crates/ulo/src/runtime.rs:45`; `Link::usable`, `Link::prepare` `link.rs:60`; `Engine::playground_html` `crates/ulo-graphql/src/lib.rs:37`; `Harness::stoppable` `crates/ulo-runtime-conformance/src/lib.rs:55` | override the method | caller of the method | the default body (`None`, `"unknown"`, `Ok(())`) | no; `Spawn::name` yields a name a refusal prints | the body |
| S5 | Required predicate method plus a reason method with a default | `Store::create_connects(o)`, `Store::connects_because(o)` `crates/ulo-db/src/store.rs:29,34` | implement both for the store | `StoreModule::register` `crates/ulo-db/src/module.rs:244-247` | the predicate has no default, so it cannot be left out; the reason defaults to a generic text | yes, per options value | none for the predicate |
| S6 | Optional sub-trait that gates a method by bound | `MigrationLock: Store` `crates/ulo-db/src/migrate.rs:47`, gating `StoreModule::migrate` `module.rs:155` | `impl MigrationLock for Postgres` | the compiler, at the call site | `.migrate(..)` does not exist for that store; a compile error | no | absent |
| S7 | Refusal returned by a probing call | `Tokio::require_io` `crates/ulo-runtime-tokio/src/lib.rs:91`; `Timer::try_sleep` and `Refused` `crates/ulo/src/timer.rs:36,42`; `NoTimer` `link.rs:494`; `UnsupportedRuntime` `crates/ulo-db/src/connect.rs:28` | nothing declared; the environment is probed | the link or module at `usable`/`prepare`/`listen`/`connect` | `Err` with a typed error whose `Display` names the cause | yes, in the error text | none |
| S8 | Absence declared in the conformance suite, with a reason | `conformance_suite!(.. not_applicable { name: "reason" })` `crates/ulo-rpc-conformance/src/lib.rs:187`; same macro in `ulo-http-conformance:329`, `ulo-ws-conformance:245`, `ulo-runtime-conformance:98`, `ulo-db-conformance`; `Host::upgrades() -> bool` and `without_upgrades` `ulo-ws-conformance/src/lib.rs:124,228` | the link's `tests/conformance.rs` | the macro, which stamps `#[ignore = "not applicable: .."]` | the test is ignored, and fails if run | yes, a required string literal | none |

An app-side builder knob sits beside S1 on HTTP: `Embedded::peer_addr(true)` (`embed.rs:427`) overrides a host's `peer_addr: false`. Link constructors carry others (`.codec(..)`, `.max_frame(..)`, `.with_handle(..)`); they set configuration that the link then reports through S1 (below), and are not a declaration shape of their own.

## The shapes, with source

### S1. Public-field struct with const setters

Declaration (RPC), `crates/ulo-rpc-udp/src/link.rs:197-208`:

```rust
fn capabilities(&self) -> Capabilities {
    Capabilities::new(DeliveryMode::Addressed)
        .binary(self.codec.binary())
        .max_frame(Some(MAX_DATAGRAM))
        .ordering(ulo_rpc::Ordering::Unordered)
        .shapes(UNARY_ONLY)
        .miss_signal(true)
        // `close` answers what its socket holds; ...
        .confirms_drain(false)
}
```

Declaration (HTTP), `crates/ulo-http-actix/src/lib.rs:76-85`:

```rust
fn limits() -> EmbedLimits {
    EmbedLimits::NONE
        .upgrades(false)
        .forward_miss(false)
        .host_extensions(false)
        .tls_info(false)
        .disconnect(Disconnect::AtNextWrite)
        .drain_pending(DrainPending::Closed)
        .drain_http2(DrainHttp2::Reset)
}
```

Read, `crates/ulo-rpc/src/server.rs:169-176` and `crates/ulo-http/src/server.rs:413-419`:

```rust
if !capabilities.shapes.contains(&shape) {
    ... format!("{} (pattern `{pattern}`) is a {} handler, which the {} link does not carry", ..)
```
```rust
if upgrading && !limits.upgrades {
    failures.push(format!(
        "the {} backend declares `upgrades: false`, so it cannot hand a WebSocket upgrade to a gateway on its port; \
         serve the gateways on a separate port",
        B::NAME
    ));
}
```

Facts checked:

- The struct is `#[non_exhaustive]` (`link.rs:271`, `backend.rs:60`) so a link builds it through `new` or `NONE` and the setters.
- Polarity differs. RPC `Capabilities` mixes defaults: `confirms_drain` is `true`, `holds_unserved`, `durable_replies`, `ordered_control`, `miss_signal`, `native_backpressure` are `false` (`link.rs:334-349`). HTTP `NONE` sets every boolean `true` and a link subtracts (`backend.rs:76`, `embed.rs:163-174`). Both follow the §3.9 rule (see below) from different directions.
- Some fields are enums with variants that refuse nothing (`EmbedLimits::request_body`, `disconnect`, `drain_pending`, `drain_abandoned`, `drain_http2`; their doc comments say "Nothing is refused"). RPC `ordering` and `delivery` are enums the suites read.
- Some RPC flags are read only by the conformance suite and not by core: `holds_unserved` (`cases/events.rs:51`), `durable_replies` (`cases/recovery.rs:28`), `ordered_control` (`cases/order.rs:148`). Core reads `shapes`, `binary` (`server.rs:169,180`, `codec.rs:47`), `delivery` (`dispatch.rs:792`), `max_frame` (docs at `client.rs:452`, `server.rs:99`) and `fan_out_reason` (S2). Which of `native_backpressure`, `miss_signal`, `confirms_drain` core reads was not traced: unverified.
- `max_frame` is computed from the link's configuration at call time: NATS reads an atomic written after the server's `INFO` (`crates/ulo-rpc-nats/src/link.rs:293-300`), TCP reads its builder (`crates/ulo-rpc-tcp-core/src/link.rs:200-206`). `capabilities(&self)` therefore takes `&self`; a value that depends on config is expressible here.

### S2. Private fields with a reasoned setter

Declaration, `crates/ulo-rpc-redis/src/link.rs:104,111-118`:

```rust
const FAN_OUT: &str = "Redis Pub/Sub delivers a request to every server instance subscribed to its pattern's channel";
...
Capabilities::new(DeliveryMode::FanOut)
    .binary(self.codec.binary())
    .ordering(Order::PerChannel)
    .miss_signal(true)
    .ordered_control(true)
    .fans_out_requests(FAN_OUT)
```

Definition, `crates/ulo-rpc/src/link.rs:405-407,428-436`:

```rust
pub const fn fans_out_requests(self, reason: &'static str) -> Self {
    Capabilities { delivers_requests_once: false, fan_out_reason: Some(reason), ..self }
}
pub const fn delivers_requests_once(&self) -> bool { self.delivers_requests_once }
pub const fn fan_out_reason(&self) -> Option<&'static str> { self.fan_out_reason }
```

Read, `crates/ulo-rpc/src/server.rs:205-215`:

```rust
if let Some(reason) = capabilities.fan_out_reason()
    && requests > 0
{
    tracing::warn!(link = L::NAME, request_patterns = requests, reason, "the {} link delivers each request to every server instance ...", L::NAME);
}
```

Facts checked:

- The two fields are private (`link.rs:318-319`); a `compile_fail,E0616` doctest at `link.rs:422-428` pins that writing `delivers_requests_once = false` from outside the crate does not compile. The Q22-P7 caveat that "a link can set the bool false without a reason" no longer holds at this commit.
- The only consequence of `false` is a `warn` from `Server::prepare` when the server mounts at least one request pattern; nothing is refused (`server.rs:205-215`). The conformance case branches on the accessor (`crates/ulo-rpc-conformance/src/cases/delivery.rs:79`).
- Only Redis declares it (`grep fans_out_requests crates` finds `ulo-rpc-redis/src/link.rs:117` and a test in `crates/ulo-rpc/tests/fan_out.rs:150`).
- `DeliveryMode` (`link.rs:440-449`, `Competing | FanOut | Addressed`) also describes how a request reaches two instances; the Q22-P7 note that its documentation overlaps stands. For Redis the two declarations agree: `DeliveryMode::FanOut` and `fans_out_requests`.
- `confirms_drain(false)` is a sibling "false is the exception" capability (S1) that carries its reason only as a source comment (UDP, above), not as a value.

### S3. Associated const with a default

Declaration, `crates/ulo-http-actix/src/lib.rs:72`:

```rust
const STRIPS_PREFIX: bool = false;
```

Default and read, `crates/ulo-http/src/embed.rs:94,1036`:

```rust
const STRIPS_PREFIX: bool = true;
...
if !A::STRIPS_PREFIX {
```

Also `Transport::READS_PREFIX: bool = false` and `BUILDS_CONTROLLER: bool = true` (`crates/ulo/src/transport/mod.rs:47,55`), read at `crates/ulo/src/transport/controller.rs:53` and `pipeline.rs:207`. The doc comment on `READS_PREFIX` states the polarity rule: "`false` unless the transport says otherwise, so a transport that joins no prefix needs no declaration to have one refused". A const is usable in const contexts and is not per-instance: it cannot vary with configuration.

### S4. Trait method with a default, or returning `Option`

Declaration and default, `crates/ulo-http/src/embed.rs:112-115` (read by `Service` at `embed.rs:1104`):

```rust
fn take_upgrade(extensions: &mut http::Extensions) -> Option<OnUpgrade> {
    let _ = extensions;
    None
}
```

`Spawn::name` (`crates/ulo/src/runtime.rs:45`) defaults to `"unknown"`, which `ulo-db` reads to refuse a runtime (`crates/ulo-db/src/connect.rs:28-31`, S7). `Engine::playground_html` returns `Option<String>`, `None` meaning "serves no page" (`crates/ulo-graphql/src/lib.rs:37`), read at `crates/ulo-graphql-http/src/playground.rs:21`. `Harness::stoppable() -> Option<Stoppable<_>>` makes `None` mean "cannot shut down" and the suite then fails the scenario unless S8 declares it. A method can compute from `&self` and its arguments; the default is a decision for every implementor that says nothing. Note the asymmetry inside this shape: a default of `None`/`"unknown"` here is the permissive reading, where §3.9 asks for the strict one. For `take_upgrade` the strictness is carried instead by `EmbedLimits::upgrades` (S1), which must be `true` for the method to be consulted (`embed.rs:110-111` doc comment).

### S5. Required predicate with a reason method

Declaration, `crates/ulo-db-mongodb/src/store.rs:75-81`:

```rust
fn create_connects(o: &MongoOptions) -> bool {
    o.uri.expose().starts_with(crate::options::SRV)
}

fn connects_because(_: &MongoOptions) -> &'static str {
    "a `mongodb+srv://` URI resolves its DNS inside `create`, which the retry loop must carry; use a `mongodb://` URI to be lazy"
}
```

Read, `crates/ulo-db/src/module.rs:244-247`:

```rust
let connects = validated.as_ref().is_ok_and(|options| S::create_connects(options));
if connects && self.lazy.is_some() {
    let because = validated.as_ref().map_or("", |options| S::connects_because(options));
    m.try_value::<LazyRefused, _>(Err(LazyRefused::new(S::NAME, because)));
}
```

The predicate has no default (`store.rs:29`), so a store cannot omit the declaration; the other stores answer `false` or `true` without a reason (for example `crates/ulo-db-diesel/src/store.rs:184`). The reason method defaults to ``"its `create` connects"`` (`store.rs:34-37`) and is read only when the predicate is `true`. The reason depends on the options value, which S1 and S2 cannot express per value.

### S6. Optional sub-trait

Declaration, `crates/ulo-db-diesel/src/store.rs:246`:

```rust
impl MigrationLock for Postgres {
    type Guard = DieselLock;
    ...
```

Read, `crates/ulo-db/src/module.rs:155`:

```rust
impl<S: MigrationLock> StoreModule<S> {
    ... pub fn migrate(mut self, migrations: impl Migrations<S>) -> Self {
```

Whether a store supports the capability is a type-level fact; the absence is a compile error with no custom message unless `#[diagnostic::on_unimplemented]` is applied to `MigrationLock` (unverified). The capability carries its own associated type (`Guard`) and methods, which S1 to S5 cannot.

### S7. Refusal returned by a probing call

Probe, `crates/ulo-runtime-tokio/src/lib.rs:91-100`:

```rust
pub fn require_io(&self) -> Result<(), NoIoDriver> {
    let enabled = match self.io.get() {
        Some(enabled) => *enabled,
        None => match probe_io(&self.handle) { Some(enabled) => *self.io.get_or_init(|| enabled), None => true },
    };
    if enabled { Ok(()) } else { Err(NoIoDriver) }
}
```

Read, `crates/ulo-rpc-tcp/src/link.rs:113-114`: `self.runtime()?.require_io()?;` inside `Link::usable`. A server link with no app timer returns `Err(NoTimer { link: "TCP" }.into())` from `listen` (`crates/ulo-rpc-tcp-core/src/link.rs:266`). A runtime that has shut down answers `Err(Refused)` from `Timer::try_sleep` (`timer.rs:36`). The runtime does not declare "I have IO"; the caller asks and gets a typed error. Its text carries the cause and the remedy (`NoTimer`'s `Display`, `link.rs:498-502`). The answer can change over time (`Refused` after shutdown), which a declaration cannot.

### S8. Absence declared in a suite, with a reason

Declaration, `crates/ulo-rpc-kafka/tests/conformance.rs:148-150`:

```rust
ulo_rpc_conformance::conformance_suite!(KraftBroker; not_applicable {
    cancel_follows_its_request: "U17: the link carries a request and its `cancel` on separate lanes, and the broker may deliver the `cancel` first",
});
```

Read, `crates/ulo-rpc-conformance/src/lib.rs:187-205`: the macro stamps `#[ignore = concat!("not applicable: ", $why)]` for each declared name and `const _: fn() = $skip;` so a name that is no scenario fails to compile. The reason is a required `literal`. The scenario `cancel_follows_its_request` reads `linked.server.capabilities().ordered_control` (`cases/order.rs:148`), so S8 and S1 declare the same fact twice: Kafka's `capabilities()` does not set `ordered_control` (`crates/ulo-rpc-kafka/src/link.rs:217-225`), so it stays false (S1) and lists the scenario here. WebSocket has no capability struct; the suite's `Host::upgrades()` (`crates/ulo-ws-conformance/src/lib.rs:124`) is a required trait method that the scenario `as_declared` checks against the real behaviour (`cases/handshake.rs:44,72`), and `without_upgrades` turns every other scenario not applicable. The runtime suite does the same with `Harness::stoppable` returning `None` (S4) and `not_applicable` entries in `crates/ulo-runtime-smol/tests/conformance.rs:32-38`.

## What each shape cannot express

- A reason (S1, S3, S4 except via doc text): a `false` in a public-field struct is a bare bool. RPC `confirms_drain(false)` on UDP and NATS has its reason only in a source comment and in `transports/DESIGN.md:1085`. HTTP `upgrades(false)` on actix has none in the declaration (the refusal message names the limit but not why the host lacks it; reasons per host are in crate docs, unverified for each).
- A value that depends on configuration (S3): an associated const is the same for every instance. S1 can (via `&self`, as `max_frame` does), S5 can (via the options argument), S2 can only if the setter is called conditionally.
- A reason that depends on the value (S1, S2): `fan_out_reason` is one `&'static str` per link; S5's reason takes the options.
- A capability with its own types and methods (S1 to S5): a bool or enum cannot carry `MigrationLock::Guard`; S6 can.
- A change over time (S1 to S6): a runtime that shuts down, or an IO driver that was never enabled, is S7's.
- A compile-time absence (S1 to S5, S7, S8): they all fail at `prepare`, at `connect`, at test time or at run time. Only S6 fails at compile time.
- An arbitrary number of undeclared capabilities with a strict default (S4): a defaulted method is the permissive reading for every implementor that says nothing.
- A generic reader (all): no two of S1 to S8 share a trait or a type, so nothing can ask "what can this host or link do" across HTTP, RPC and WebSocket. Per `build-sequence.md:446-448` no code at HEAD needs to; unverified beyond the search noted at the top.
- S8 cannot refuse a user's app; it constrains only the implementor's own test run.

## The rule in `DESIGNER_HANDOFF.md` §3.9

`docs/experiments/di-redesign/DESIGNER_HANDOFF.md:205-211` (§3.9, "The conformance suites are the public contract"):

> Hosts and links declare capabilities. A capability is named as a positive property, and defaults to the strict reading (for example `confirms_drain` defaults to `true`), so a capability left undeclared fails a test rather than loosening one. `not_applicable` needs a stated reason, and is never used to make a run pass.

`transports/DESIGN.md:1085` adds the rule S2 implements: "a `false` always carries its reason as a strict reading's exception does (Q22-P7)". Against the shapes:

- S2 and S8 enforce it in the type: a reason is a required argument.
- S5 enforces it for one capability: the predicate is required, and the reason method has a default text, so the reason can be generic but not absent.
- S1 does not: `confirms_drain(false)` (UDP, NATS), `holds_unserved`, `durable_replies`, `ordered_control` and every HTTP `limits()` entry can be set without a reason. The strict default applies (RPC `confirms_drain` and `delivers_requests_once` start `true`; HTTP starts all `true`) but the exception is unexplained in the value.
- S3, S4: the default is the permissive reading in several places (`READS_PREFIX = false` is the strict one by its doc comment; `take_upgrade -> None` and `Spawn::name = "unknown"` are not obviously either). Unverified whether §3.9 is meant to reach these.
- S6 is strict by construction: a store that does not implement the trait cannot call the method.
- The rule says "positive property". `fans_out_requests` is a verb that sets a negated property (`delivers_requests_once = false`); the accessors name the positive one.

## Recommendation (no decision made here)

Each choice below is described by what it would change.

1. Keep all eight as they are and record that the choice was made (the earlier recommendation, `build-sequence.md:446`, `C-X17`). No code changes. The cost is that §3.9's reason rule stays enforced for S2, S5 and S8 only, and a new SPI has eight precedents to choose from.
2. Converge "declares what it can do, read at `prepare`" on S1. This touches S3 (`Embed::STRIPS_PREFIX` could become an `EmbedLimits` field; `Transport` consts are about transports, not links, and need not move), the `Option`-returning members in S4 and `Harness::upgrades`. It gives one place to read a host's or link's abilities. It keeps S5, S6 and S7, which S1 cannot replace (they depend on options, carry types, or probe an environment). It removes the doc-only polarity rule on S3 and puts the `NONE`/`new` split (HTTP all `true`, RPC mixed) in front of the designer: one default direction would have to be chosen.
3. Extend S2's reason rule to every S1 exception. Mechanically: each negated flag gets a private field, a reasoned setter and accessors, as `fans_out_requests` does; or the setters for exception flags take a reason argument (for example `confirms_drain(false, "..")`). This puts a reason where `confirms_drain(false)`, `upgrades(false)`, `host_extensions(false)` and `forward_miss(false)` now have none, and the `warn`/refusal can print it. The cost is a larger public surface per flag (three items instead of one), a breaking change to every link's `capabilities()` and every host's `limits()`, and the question of which flags are exceptions: on HTTP `NONE` makes every `false` an exception, on RPC only some are. The Q22-P7 note that S2 changes nothing but a `warn` also applies: a reason makes a declaration visible, not enforced.
4. Add a shared read trait over S1 structs (for example one that lists `(name, bool, reason)`). Nothing at HEAD consumes it, so the gain would be tooling (a `ulo` command, a docs table generated from declarations) rather than behaviour. Unverified whether the docs site wants that.
5. Move S8's reason into S1 and drop the duplicate declaration (for example `cancel_follows_its_request` is skipped because `ordered_control` is false). The suite could skip from the declared capability and print its reason, the way `without_upgrades` already skips from `Host::upgrades`. That removes the second place a link must say the same thing and the possibility of the two disagreeing, and loses the property that the suite fails when a link leaves out a declaration it needs (`conformance_suite!` doc comment, `lib.rs:181-186`), since the skip would follow the link's own claim.

## Not verified

- Which of the ledger's "four" shapes QS-19 meant: no document enumerates them (searched `docs/experiments`, `docket/`).
- Whether core reads RPC `native_backpressure`, `miss_signal` and `confirms_drain` outside the conformance suite (`grep` finds reads of `shapes`, `binary`, `delivery`, `fan_out_reason` and, by documentation, `max_frame` in `ulo-rpc`; the other three were not traced to a reader).
- Whether `MigrationLock` carries `#[diagnostic::on_unimplemented]`.
- Whether any docs page outside `docs/experiments` describes these shapes.
- The WebSocket SPI declares nothing beyond S8's `Host::upgrades` in what was read (`ulo-ws`, `ulo-ws-hyper`); if a WebSocket capability exists in `ulo-ws-hyper` under another name it was not found.
