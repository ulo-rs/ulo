# Divergences: the race 3 spine, S-core

Every place S-core departs from `integrations/DESIGN.md`, `BUILD_PLAN_3.md` or the fourth and fifth
responses, or fills a shape they leave open. Each entry gives what the design says, what the code
does, and why. All await the user's sign-off. S-move and S-skel log in `race3-S-move.md` and
`race3-S-skel.md`.

No cargo was run. Every claim about what compiles, and every library fact, is read from the sources
and from the registry under `~/.cargo/registry/src`; what only a compile or a run can show is under
"Not verified" at the end.

S-core wrote 35 files: 22 of finished code and 13 at the insertion points. The plan counts 36; the
one not written is `crates/ulo-ws/src/gateway.rs` (entry C17). Nothing of `master` is replaced: every
change is additive on the race 2b tree.

## The surface, as written

```rust
// ulo
pub trait Spawn { fn spawn(..) -> TaskHandle; fn name(&self) -> &'static str { "unknown" } }
#[non_exhaustive] pub struct Permanent(BoxError);          // Debug, Display (inner), Error (source = inner)
impl Permanent { pub fn new(error: impl Into<BoxError>) -> Permanent; }
pub fn is_permanent(error: &StartupError) -> bool;

#[cfg(feature = "serde")] impl<'de, T: Deserialize<'de>> Deserialize<'de> for Secret<T>;
pub mod __private {                                         // doc-hidden
    pub fn collect_secrets<R>(f: impl FnOnce() -> R) -> (R, CollectedSecrets);
    #[derive(Clone, Default)] pub struct CollectedSecrets;  // Debug writes the count alone
    impl CollectedSecrets {
        pub fn register(self, m: &mut ModuleDef<'_>); pub fn append(&mut self, other: CollectedSecrets);
        pub fn texts(&self) -> &[String]; pub fn len(&self) -> usize; pub fn is_empty(&self) -> bool;
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)] pub enum TraceMode { #[default] On, Off }
#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub enum TraceOrigin { Received, Originated }
#[derive(Clone, Debug, PartialEq, Eq)] pub struct TraceContext;
impl TraceContext {
    pub fn extract<'a, 'b>(traceparent: impl IntoIterator<Item = &'a str>,
                           tracestate: impl IntoIterator<Item = &'b str>) -> Option<TraceContext>;
    pub fn new(trace_id: [u8; 16], span_id: [u8; 8], flags: u8, state: TraceState) -> Option<TraceContext>;
    pub fn trace_id(&self) -> [u8; 16]; pub fn span_id(&self) -> [u8; 8]; pub fn flags(&self) -> u8;
    pub fn sampled(&self) -> bool; pub fn state(&self) -> &TraceState; pub fn origin(&self) -> TraceOrigin;
    pub fn traceparent(&self) -> String; pub fn tracestate(&self) -> Option<String>;
}
#[derive(Clone, Debug, Default, PartialEq, Eq)] pub struct TraceState;   // Display: the header value
impl TraceState {
    pub fn new() -> TraceState; pub fn parse<'a>(fields: impl IntoIterator<Item = &'a str>) -> TraceState;
    pub fn entries(&self) -> impl Iterator<Item = (&str, &str)>; pub fn get(&self, key: &str) -> Option<&str>;
    pub fn len(&self) -> usize; pub fn is_empty(&self) -> bool;
    pub fn with_entry(&self, key: &str, value: &str) -> Option<TraceState>;
    pub fn prepend(&self, front: &TraceState) -> TraceState;
}
pub trait TraceSource: Send + Sync + 'static {
    fn start(&self, span: &tracing::Span, inbound: Option<&TraceContext>) -> TraceContext;
}
impl ExecOptions { pub fn trace(self, ctx: TraceContext) -> Self; pub fn span(self, span: tracing::Span) -> Self; }
impl Execution { pub fn trace(&self) -> Option<&TraceContext>; pub fn set_trace(&self, ctx: TraceContext); }
impl ExecutionRef { pub fn trace(&self) -> Option<&TraceContext>; }
impl AppBuilder { pub fn trace(self, mode: TraceMode) -> Self; }
impl AppHandle {
    pub fn trace_source(&self) -> Option<&Arc<dyn TraceSource>>;
    pub fn trace_mode(&self) -> TraceMode;                  // added, C9
}
impl<S> TestApp<S> { pub fn trace(self, mode: TraceMode) -> TestApp<Settled>; }

// ulo-transport
pub mod span {
    pub const OTEL_KIND: &str = "otel.kind"; pub const OTEL_STATUS_CODE: &str = "otel.status_code";
    pub const STATUS_ERROR: &str = "error"; pub const STATUS_OK: &str = "ok";
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)] pub enum Kind { Server, Consumer, Client, Producer, Internal }
    impl Kind { pub fn as_str(self) -> &'static str; }
    pub fn call(app: &AppHandle, exec: &Execution, kind: Kind, transport: &'static str,
                name: &str, handler: Option<&str>) -> Span;
}

// ulo-grpc
impl GrpcHealth {
    pub fn module() -> DynamicModule; pub fn delegate_overall(&self);
    pub async fn set<S: NamedService>(&self, status: ServingStatus);
    pub async fn set_named(&self, name: &str, status: ServingStatus);
}

// the hubs' insertion points, `pub(crate)`, bodies `todo!()` for O1
ulo_http::trace::extract(&HeaderMap) -> Option<TraceContext>
ulo_grpc::trace::extract(&HeaderMap) -> Option<TraceContext>
ulo_grpc::trace::inject(&TraceContext, &mut MetadataMap)
ulo_rpc::trace::extract(&CallHeaders) -> Option<TraceContext>
ulo_rpc::trace::inject(&mut CallHeaders, &TraceContext)
ulo_ws::trace::extract(&HeaderMap) -> Option<TraceContext>
// each beside `pub(crate) const TRACEPARENT: &str = "traceparent";`
```

## Entries

C1. **`is_permanent` answers `true` for every `StartupError::Wiring`, and for `Bind` carrying
`RuntimeMissing`.** The design gives `is_permanent` as "walking the chain as `is_panic` does", which
finds a `Permanent`; principle 2 and decision 40 say the function is how `main` tells a
misconfiguration from a retryable failure, and that a wiring refusal is a misconfiguration "by the
first phase that can see it". A walk alone answers `false` for a configuration source that failed to
load, which would have an orchestrator restart a process that can never start. `Connect`, `Configure`
and `Bind` are walked: each `Redacted` reason, each server's `prepare` failure and each bind error.
`crates/ulo/src/error/mod.rs`.

C2. **The walk enters a `Redacted`'s original and follows its `source()` chain.** `is_panic` inside a
`Redacted` recognises only the core's own types, since `Redacted` hands its original out by type
alone. The plan asks `Permanent` to be recognised inside a `Redacted` "as one of the core's own
types", which a top-level downcast covers; a user's error carrying a `Permanent` as its source would
still be missed. S-core added `Redacted::original(&self) -> &(dyn Error + 'static)`, `pub(crate)`,
and the walk follows the original's chain under the same depth bound (32). `crates/ulo/src/redact.rs`.

C3. **A readiness attempt stops on a `Permanent` under `ConstructError::Dependency` too.** I2 names
the `Failed` arm. An attempt whose dependency read failed with a construction that returned a
`Permanent` (`LookupError::Construct { reason: Errored(..) }`) is equally beyond retrying, so the same
walk is used for both arms (`construct_permanent`, `pub(crate)`). The check ends as
`ConnectError::Readiness { reason: Errored(..) }`, the attempt count the attempts made, no backoff.
`crates/ulo/src/lifecycle/connect.rs`.

C4. **What the collector records, for a `T` that is not `Display` (plan, "points S-core resolves").**
`Secret::deserialize`, while a collection is open on the thread, deserialises `T` through a wrapper
around the deserializer it was given, which records the text of every scalar `T`'s `Deserialize`
visits: strings, numbers and chars by their text, bytes when they are UTF-8. So a `Secret<T>` is
recorded whatever `T` is, by the text it was read from, and `CollectedSecrets::register` calls
`m.secret(&Secret::new(text))` for each. Its limits, stated on the impl's module docs:
- nothing inside a sequence, a map or an enum is recorded: a `Secret<Vec<String>>` under `flatten`
  is not collected (the record by name still holds it outside `flatten` and `untagged`);
- nothing under a request for an `Option`: serde answers a flattened `Option` through
  `Visitor::__private_visit_untagged_option`, which is "not public API" (`serde_core-1.0.229
  src/de/mod.rs:1712-1719`), and a wrapper that does not forward it would break that case, so the
  wrapper passes an `Option` request through unwrapped. `Option<Secret<T>>`, the usual spelling, is
  unaffected; `Secret<Option<T>>` under `flatten` is not collected;
- a `bool` and a unit are not recorded: as redaction texts they would replace every `true`;
- a value is recorded as it is visited, before `T` has accepted it, so a value `T` then refuses is
  recorded too: more redaction, never less;
- a collection opened inside another collects for itself alone; the outer one receives nothing.
`crates/ulo/src/redact.rs`, `crates/ulo/src/__private.rs`.

C5. **`CollectedSecrets` carries `Clone`, `Default`, `append`, `texts`, `len`, `is_empty` and a `Debug`
writing the count.** The fixed surface names `register(self, m)` alone. `texts` and `Clone` answer
S-skel's R1 (`Section<T>` is `Clone + Eq + Hash` and keeps its secrets as `Vec<Secret<String>>`);
`append` serves a caller running several deserialisations.

C6. **The collector is not behind the `serde` feature; the `Deserialize` impl is.** Without the
feature `collect_secrets` runs `f` and answers an empty collection, so a crate naming it compiles
either way. `ulo-config` enables the feature in its own manifest.

C7. **I12's help text and preference.** The help reads ``help: {exporter} exports `{bound}`, the same
type from two versions of its crate; `cargo tree -d` lists them``. `near_spelling` names a key of
the same full name before one alike by its last segment, among the visible keys and then across the
graph, since two versions of a crate is the likelier cause of a key missing beside an equal name.
The `WiringError::Missing` variant is unchanged: the rendering compares the two keys' `TypeId`s and
full names (`two_versions`, `pub(crate)` in `graph/visibility.rs`). `spelled_alike` now also matches
equal full names with different `TypeId`s, which `own_near_spelling` (the export hint) shares.

C8. **`TraceContext`'s methods beyond `origin` (plan, "points S-core resolves").** As in the surface
above. What each decides:
- `extract` takes every field value under each name, so a duplicated `traceparent` is a restart and
  several `tracestate` fields are one list. The text kept "as received" is each value trimmed of the
  space and tab around it, which RFC 9110 does not count as part of a field value, and the
  `tracestate` fields joined with `,`, as a recipient may combine them.
- A `Received` context with that text writes it unchanged from `traceparent()` and `tracestate()`,
  a higher version and Level 2's `random` flag included. A `tracestate` that failed to parse is
  forwarded as received too, the parsed `state()` then empty: §3.4 forbids modifying a list passed
  through with its `traceparent`.
- Any other context, an `Originated` one or one built by `new` and handed in through
  `ExecOptions::trace`, is written as version `00` with the sampled flag as its one flag (Level 1).
- `flags()` answers the bits as received or as the source gave them; the masking is on output only.
- `set_trace` drops the received text: it describes the hop before this one.
- `new` refuses all-zero IDs, answering `None`.
- `TraceState::prepend(front)` is the merge §6.3 gives the client for an `Originated` context: the
  caller's entries at the left in their order, a shared key moving left with the caller's value,
  then truncation by whole entries, over 128 characters first, then from the right, the caller's
  entries the last removed and the leftmost never.
`crates/ulo/src/execution/trace.rs`.

C9. **`TraceMode::Off` is enforced by the core, and `AppHandle::trace_mode()` is added (plan, "how a
hub reads `TraceMode`").** Under `Off`: `Execution::open` drops a context in `ExecOptions::trace`,
`set_trace` changes nothing, and `connect` resolves no `TraceSource`, so `trace_source()` answers
`None` and `span::call` calls nothing; an execution then never carries a context, and the RPC
client's reservation and injection never apply. A hub reads `trace_mode()` only to skip parsing
headers it would drop; correctness does not depend on it. `trace_mode()` is an accessor beside
`trace_source()` that no skeleton names.

C10. **`set_trace` takes effect only before `dispatch` begins, enforced.** I13 says "before
`dispatch`, as `route_to` does"; `route_to` states it and does not enforce it. `set_trace` reads the
`handler` slot `dispatch` sets before the first guard and changes nothing once it is set, so a guard,
an interceptor or a handler never sees the execution's context change.

C11. **`execute` runs the closure inside the span `ExecOptions::span` hands it (plan, "points S-core
resolves").** With a source bound, `start(&span, inbound)` is called before the span is first
entered and the answer recorded through `set_trace`; the closure then runs instrumented with the
span. With no source, or under `Off`, the span is still entered around the closure: a caller who
created a span for a job means the job to run in it. `crates/ulo/src/app/shared.rs`.

C12. **A `dyn TraceSource` that cannot be read at `connect` fails the connect.** The design resolves
`Option<Dep<dyn TraceSource>>` and is silent on an error other than `NotFound`: one two visible
exports answer (`Ambiguous`), one bound per execution (`ExecutionRequired`), one whose construction
failed. Each fails `connect` as `ConnectError::Construct { key: dyn TraceSource, module: <root> }`,
its reason the construction's own or the lookup error's text. Running on without the source would
leave tracing off with no word of why. The source is the one visible from the root module, the key
exactly `dyn TraceSource` (no `+ Send + Sync`), which O2's module binds and exports.
`crates/ulo/src/app/mod.rs`.

C13. **`span::call` takes the span's kind and writes `otel.kind` at creation.** The design and the plan
have `call` declare `otel.kind` empty "for each hub to record". `tracing-opentelemetry` 0.34.0 fixes a
span's name and kind when the OpenTelemetry span starts: after that, `on_record` reaches
`SpanBuilderUpdates::update_span`, which applies the status and attributes alone
(`tracing-opentelemetry-0.34.0/src/layer.rs:242-253`, `:1288-1291`). `TraceSource::start` starts
the span inside `call` (it reads `span.context()` back, PROBES_obs.md §2), so a kind recorded by the
hub after `call` returns would never reach the exporter. `call` therefore takes a `Kind`, a new
public enum in `ulo_transport::span`, and records it in the macro; `otel.status_code` stays declared
empty, and a status recorded later does reach the span. Call sites: HTTP, gRPC, a WebSocket connect
and message `Server`; an RPC request `Server`, an RPC event `Consumer` (§6.2). O1 decides the
non-HTTP kinds and may change those arguments.

C14. **`span::call`'s parameters (plan, "points S-core resolves").** `call(app: &AppHandle, exec:
&Execution, kind, transport, name, handler)`: the source is read through `app.trace_source()`, the
inbound context through `exec.trace()`, and the answer written through `exec.set_trace`. A span
opened a second time for one execution starts on the execution's own context, its first span's, as
`exec.trace()` answers it once set.

C15. **`GrpcHealth`'s lock and writes (plan, "the lock `GrpcHealth`'s stored copy and its writes
share").** A `tokio::sync::Mutex<HashMap<String, ServingStatus>>` shared by every clone, starting
with `""` at `SERVING`, held across the comparison and the reporter's write; tonic-health 0.14.6's
`set_service_status` awaits its own `RwLock` (`src/server.rs:74-92`), so the lock must be held
across an await, and the crate is tokio-bound already. The copy records a status after the reporter
has taken it. The server's writes at `bind` and the drain go through `set_by_server`, `pub(crate)`,
which skips `""` once `delegate_overall` has run; `set_named("")` stays open, which is how the bridge
writes `""`. `delegate_overall` is an `AtomicBool` and may be called at any time; its docs say
before `listen()`. `crates/ulo-grpc/src/health.rs`, `server.rs`.

C16. **The RPC reservation matches the trace headers' names without regard to case.** `deadline-ms`
stays an exact match, as it is today; W3C Trace Context matches its header names case-insensitively,
and a caller's `Traceparent` beside the client's `traceparent` would reach a server as two fields,
which the specification makes a restart. `deadline-ms` is checked first, then `traceparent`, then
`tracestate`. The `tracestate` refusal's message names `ulo-otel`'s source as the way to add a vendor
entry. The F400 test (`client.rs:1635-1645`) sends `traceparent` on calls with no `.within`; no
context, no reservation, and the test holds as written. `crates/ulo-rpc/src/client.rs`.

C17. **`crates/ulo-ws/src/gateway.rs` is not edited.** The plan lists `gateway.rs:576` as a
hand-written gateway's execution. Line 576 is `resolve`, which opens a short execution in the
gateway's module to resolve `G` for `after_init` and the envelope gateway's hooks; it runs once per
gateway, with no connection and so no connection context. The hand-written gateway's execution is
`Connection::open_execution` (`connection.rs:143`), which now opens with the connection's context.

C18. **The connection's context and `Connection::exec_options` (plan, O1's "where the WebSocket
connection keeps its context", set up here since the call sites are S-core's).** `ConnInner` gains
`trace: OnceLock<TraceContext>`, set right after the connect phase's `span::call` from
`exec.trace()`, the connect execution's own context with a source and the handshake's without.
`Connection::exec_options()`, `pub(crate)`, opens every later execution, a message's, a hand-written
gateway's and the terminal `on_disconnect`'s (both its plain and its terminal open), with that context
as the inbound one; `ExecOptions::trace` marks it `Received` there, and the message's own span call
replaces it with an `Originated` one when a source is bound.

C19. **Extraction is called only when a `traceparent` field is present, and injection only when the
execution carries a context.** The plan has each hub call its helper. A request with no
`traceparent` has no context to continue (W3C §4.2 discards a `tracestate` alone), so the guard
changes no outcome; it also keeps every existing hub test, none of which sends `traceparent` to a
server, off the `todo!()` until O1 fills it. For the same reason the gRPC helper takes the context,
`inject(&TraceContext, &mut MetadataMap)`, called from `outgoing` when `exec.trace()` answers one,
and the RPC client calls `trace::inject(headers, context)` from `written` only for a call made
`.within` an execution carrying a context. `written` gains `within: Option<&ExecutionRef>` and the
injection sits ahead of its early return for a call with no bound, on both call sites.

C20. **`TestApp::trace(TraceMode)` is added (§14's question, answered yes).** `TestApp` mirrors every
other `AppBuilder` knob, and §6.5's third scenario runs a transport suite with tracing off.

C21. **`ExecOptions` keeps `Clone`, `Debug` and `Default`.** `tracing::Span` is all three, so adding the
span field changes none. Each setter now keeps the other fields, which `deadline` did not need while
it was the only one.

## Requests to other owners

None from S-core. Answers to requests made of S-core:

- S-skel R1 (read a `CollectedSecrets`'s texts): granted, `texts(&self) -> &[String]` and `Clone`
  (C5).
- S-skel R2 (redacting a `Health::Down` reason): no core change. `AppHandle::redact(err) -> Redacted`
  is public today (`crates/ulo/src/app/handle.rs:151`) and runs the graph's redaction; the core
  binds no `AppHandle` for injection, so whether `ulo-health` can reach one, through `ulo-http`'s
  handler context or otherwise, is H1's to find.

## For the areas that build on this

- **O1.** Every hub's `trace::extract` and the clients' `inject` are `todo!()`; the guards of C19
  keep them unreached by today's tests. A request carrying `traceparent` panics there until stage 5
  fills them. `otel.status_code` is recorded by each hub through `span::OTEL_STATUS_CODE` and
  `span::STATUS_ERROR`; no status helper is written in `trace.rs`. The RPC `inject` receives the
  caller's headers with any caller `tracestate` still in them, for the merge into an `Originated`
  context; the reservation has already refused one beside a `Received` context.
- **O2.** `TraceSource` is resolved as `Option<Dep<dyn TraceSource>>` from the root module once,
  at `connect`; bind it under `dyn TraceSource` and export it so the root sees it. `start` is called
  with a span not yet entered, and the context it answers is written through `set_trace`, which
  marks it `Originated`. `TraceContext::new(trace_id, span_id, flags, state)` builds the answer;
  `TraceState::parse([text])` reads OpenTelemetry's trace state header back.
- **D1.** `Spawn::name` is forwarded by the core's wrapper, so `Dep<dyn Runtime>`'s `name()` answers
  the given runtime's even after `.timer(..)`. A `create` loop tests its own error for `Permanent`
  with `downcast_ref` at the top level; `is_permanent` takes a `StartupError`.
- **C.** Install the collector around each deserialisation with `ulo::__private::collect_secrets`;
  the record by name and the collector both register, and the registry skips repeats.

## Premises hit

- **`otel.kind` after the span starts** does not reach the exporter (read, C13); `otel.status_code`
  does (read, `layer.rs:1288-1291`). Not run.
- **tonic-health's reporter has no read** (read, `tonic-health-0.14.6/src/server.rs:30-95`): holds,
  as the plan states; the stored copy is the only truth per name.
- **serde's flattened `Option` path** goes through a visitor method that is not public API (read,
  C4); the collector does not wrap it.
- **The W3C rules** follow PROBES_obs.md §5's reference implementation case by case; its 16 vectors
  are the coordinator's unit tests for `TraceContext`.

## Where the plan and the repository disagree

- `crates/ulo-ws/src/gateway.rs:576` is not a hand-written gateway's per-connection execution (C17).
- The plan's `otel.kind` "declared empty for each hub to record" cannot reach the exporter once
  `span::call` calls the source (C13).
- Every other line the plan and DESIGN §9 cite was found where they say, at `c332aa90`.

## Not verified

- That the tree compiles: no cargo was run. The edition-2024 let-chains used in the hubs are stable
  at the 1.88 floor and already used in `crates/ulo/src/app/mod.rs`.
- That `[char; 2]` as a `str` pattern (`trim_matches([' ', '\t'])`) is accepted at 1.88; it has been
  stable since 1.51.
- That macro-generated `Deserializer` and `Visitor` methods in `redact.rs` resolve `'de` at the
  invocation site; macro_rules hygiene covers local variables, labels and `$crate`, not lifetimes.
- Any runtime behaviour: the trace scenarios of §6.5, the health scenario 5 of §3.5, and the P2
  help are the coordinator's tests after the first compile.
