# Divergences: race 2b, tests batch 25, every unary RPC call carrying its timeout as `deadline-ms` and a broker link's held `cancel` living until that deadline (S11), the broker links' `ClientProbe` and the WebSocket server's `messages_read` behind a `conformance-probe` feature (S12, S9), the app's runtime documented as the core's wrapper (S1), F382 written as a rocket limit (S6), and from the forty-eighth response F384 fixed with F387 found and fixed beside it, and the first-read count's two `unsafe` calls removed (S9)

The forty-seventh response, signed off 2026-10-11, answers `race2b-tests24.md`; the forty-eighth,
signed off the same day, answers `runtime-e2.md`, and its F384 and S9 build here. A call answered by
one reply now stamps `deadline-ms` from its own timeout, the shortest of that, the calling
execution's remaining time and a `deadline-ms` the caller set, and the server, which already read
the header, ends the call there; a stream and an event stamp only the execution's time and the
caller's header, as their timeouts bound a frame's wait and a publish. NATS, RabbitMQ, MQTT and
Kafka keep a held `cancel` until the deadline its `open` carried, with `CANCEL_HOLD` left for an
`open` carrying none. `ClientProbe`, the four links' `probe()`, `MessagesRead` and
`Server::messages_read` compile only under a new `conformance-probe` feature, which the tests turn
on through a dev-dependency of each crate on itself. F384's cause is not a borrow held across an
await: both `Param` impls wrote `extract` as an `async fn`, whose future declares no `Send`, so the
handler future's check read through it with every lifetime made generic. Writing `+ Send` in the
signature fixes it for all four transports and, unasked, for F387, the call-read half of the same
class, at no cost. The first-read count copies a connection's first bytes instead of reading
through the cursor's uninitialised memory, and its module forbids `unsafe`.

Files changed: `Cargo.lock`; `crates/ulo/src/app/{mod.rs, handle.rs}` (S1);
`crates/ulo-http-rocket/src/lib.rs` (S6); `crates/ulo-transport/src/__private.rs` (F384, F387);
`crates/ulo-macro-lints/{proto/lints.proto, src/lib.rs, src/http.rs, src/rpc.rs, src/ws.rs,
src/grpc.rs, src/di.rs}`; `crates/ulo-ws-hyper/tests/attributes.rs`;
`crates/ulo-codegen-tests/{proto/counter.proto, tests/grpc.rs}`; `crates/ulo-http-conformance/src/app.rs`;
`crates/ulo-hyper-serve/src/listener.rs` (S9); for S11, `crates/ulo-rpc/src/{client.rs, transport.rs,
dispatch.rs, held.rs}`; for S11 and S12, `crates/ulo-rpc/{Cargo.toml, src/__private.rs}`,
`Cargo.toml` and `src/link.rs` of the NATS, RabbitMQ, MQTT and Kafka links,
`crates/ulo-rpc-conformance/{Cargo.toml, src/lib.rs, src/cases/app.rs, src/cases/deadlines.rs,
src/cases/order.rs}`, and the declarations in `tests/conformance*.rs` of TCP's three suites, the
smol TCP link's three, UDP and Redis; for S9 of the forty-seventh, `crates/ulo-ws/{Cargo.toml,
src/lib.rs, src/table.rs}` and `crates/ulo-ws-hyper/{Cargo.toml, src/lib.rs}`; in the workspace's
`FRAMEWORK_GAPS.md`, F387 filed as found and fixed and a note under F384. The tree is `9e01a07f`
plus this batch; `fba3cb03` and `9e01a07f`, committed by another agent during the build, touch the
two DESIGN files and `transports/RESPONSE.md` alone, and the batch started from `cd5c32dc`.

## The signatures

```rust
// ulo_rpc::__private, doc-hidden
pub const CANCEL_HOLD: Duration;            // five seconds, now the hold for an `open` carrying no `deadline-ms`
#[derive(Clone, Copy, Debug)]
pub struct HoldUntil(/* private */);        // new
impl HoldUntil {
    pub fn at_open(headers: &CallHeaders) -> HoldUntil;    // `deadline-ms` from now, or none
    pub fn remaining(&self, fallback: Duration) -> Duration;
}
#[cfg(feature = "conformance-probe")]       // was unconditional
pub struct ClientProbe { /* unchanged */ }

// ulo_rpc_nats::Nats, ulo_rpc_rabbitmq::RabbitMq, ulo_rpc_mqtt::Mqtt, ulo_rpc_kafka::Kafka, doc-hidden
#[cfg(feature = "conformance-probe")]       // was unconditional
pub fn probe(&self) -> &ClientProbe;

// ulo_ws, under `conformance-probe` alone; were unconditional
pub struct MessagesRead;                    // `get` likewise
impl GatewayDefaults { pub fn messages_read(&self) -> MessagesRead; }

// ulo_ws_hyper, under `conformance-probe` alone; was unconditional
impl Server { pub fn messages_read(&self) -> MessagesRead; }

// ulo_transport::__private, doc-hidden: both impls of `Param`
fn extract(cx: &T::Cx) -> impl Future<Output = Result<Self, BoxError>> + Send;   // was `async fn extract`

// ulo_rpc_conformance::cases, new scenarios
deadlines::timeout_is_the_server_s_deadline    // stamped `a_call_s_timeout_is_the_server_s_deadline`
deadlines::stream_timeout_is_per_frame         // stamped `a_stream_s_timeout_is_per_frame`
order::held_until_deadline                     // stamped `a_held_cancel_lasts_until_its_call_s_deadline`
```

Features, each documented as for the conformance suite or tests and not an application feature:
`ulo-rpc`'s `conformance-probe = []`; `conformance-probe = ["ulo-rpc/conformance-probe"]` on the four
broker links; `ulo-ws`'s `conformance-probe = []`; `ulo-ws-hyper`'s `conformance-probe =
["ulo-ws/conformance-probe"]`. `ulo-rpc-conformance` takes `ulo-rpc` with `conformance-probe`.

`conformance_suite!` stamps three more scenarios. No handler, module or controller signature
changes, and no macro's output changes; `Param` is what the generated code calls.

What changes in meaning without a signature: a call through `RpcClient::request`, `send_stream` or
`Call` in any form carries `deadline-ms`, five thousand at the client's default timeout, and its
handler is cancelled with `Deadline` when that passes, where before only a call made `.within(&exec)`
carried one. A `deadline-ms` the caller set is kept when it is the shortest; inside an execution it
was replaced by the execution's remaining time whatever its value. A broker link's held `cancel`
lives until its call's deadline, where it lived five seconds. A build of a broker link or of
`ulo-ws-hyper` without the feature compiles no probe, no withholding of an `opened`, and no message
count. A handler parameter whose type carries a lifetime, `Dep<dyn Timer>` or
`Json<Cow<'static, str>>`, compiles.

Private: `CallHeaders::deadline`, moved from `dispatch.rs`'s `deadline_of`; `Spec::limit` takes
`whole`; in each broker link, `ClientCall::Streaming` gains `until: HoldUntil`,
`ClientSide::expire_cancelled` takes it, and `ClientSide::fallback_hold` is new; in
`ulo-rpc-conformance`, `cases::app::{client_over, addresses}` and the scenario file's `MutedCancel`
link; in `ulo-hyper-serve`, `FIRST_READ`.

Dependencies: the four broker links and `ulo-ws-hyper` take themselves as dev-dependencies with
`conformance-probe`, which the lock records as five new edges. No package is added.

## Decisions

### 1. F384: the bound on `extract`, not a box and not a borrow

- **Where it failed.** The generated call awaits `<P as Param<T, _>>::extract(&cx)` inside the
  `Box::pin(async move { .. })` it coerces to `BoxFuture<'static, ..>`. Both `Param` impls wrote
  `extract` as an `async fn`, which declares no `Send`, so the coercion's `Send` check read through
  it into `Injected::from_call`, `Dep::from_container`, `Resolver::dep_qualified` and `instance`,
  the lifetimes in their types taken as any lifetime. `Dep<dyn Timer>`, whose object is `'static`,
  and `Dep<&'static str>` both fail; `Dep<Probe>` passes (`probes/f384-static-str-param.txt`). No
  borrow in the generated code is held across an await that matters: `&cx` is the async block's own.
- **All four transports.** HTTP, RPC, WebSocket and gRPC read every parameter through `Param`, and a
  `Dep<dyn Timer>` handler failed on each, two errors per handler (`probes/f384-all-transports.txt`).
- **How the bound was found.** The first fix boxed `Injected::from_call`'s future, and compiled. Its
  break, written as an unboxed `fn from_call(..) -> impl Future + Send`, compiled too, and so did a
  variant reading the execution inside the block; only the `async fn` as at `cd5c32dc` failed
  (`broken/superseded/01*.txt`). The box was not the fix. Each impl now writes
  `fn extract(..) -> impl Future<Output = ..> + Send { async move { .. } }`, so the check proves
  `Send` from that bound and goes no further, with nothing allocated. A comment above the two impls
  says why. clippy's `manual_async_fn` asks for the `async fn` back on both, so each carries
  `#[allow(clippy::manual_async_fn)]` with a line pointing at that comment; the lint crate's handlers
  fail to compile if the suggestion is taken.
- **F387, the call-read half.** The probe for whether call-read parameters fail the same way found
  `Json<Cow<'static, str>>` and `Query<HashMap<Cow<'static, str>, String>>` refused with
  "implementation of `Deserialize` is not general enough" (`probes/f384-fromcall-static-param.txt`).
  The `ViaCall` impl's bound fixes it (`probes/f387-explicit-send.txt`), so it is fixed here and
  filed as found and fixed.
- **Constructors were never affected.** `#[injectable]` impl constructors taking `Dep<dyn Timer>` and
  `Dep<dyn Runtime>` compiled with the `async fn` restored: the eight errors are all at handler
  attributes (`broken/01-container-extract-async-fn.txt`). They join the lint crate anyway.
- **The suites.** The HTTP suite's `/endless` takes `timer: Dep<dyn Timer>` beside `probe` again, its
  form before stage e2, and `Suite` is a unit struct. The RPC suite's six waiting handlers, `stall`,
  `hold`, `bounded`, `bounded_too`, `ticks` and `until_drain`, take it as a parameter, and the
  controllers' `timer` fields go: the field was the workaround stage e3 recorded, and each handler
  now names the one dependency it waits on. Both suites then exercise a trait-object parameter at
  run time on every host and link.

### 2. S11: the timeout is stamped where it bounds the whole call

- **Which calls.** The response's premise, that the client abandons the call at its timeout, holds
  for a call answered by one reply: `Call`'s future is bounded by the timeout from before the
  connect to the reply. A stream's `.timeout(d)` is "the longest wait for each reply frame"
  (`StreamCall::timeout`), so a stream of ticks 20 ms apart outlives any timeout, and an event's
  bounds the publish, the handler running on with no caller. Stamping either from its timeout would
  end a long stream or a slow event handler on the server while the client still reads it or never
  waited, so `Spec::limit(whole)` stamps the timeout for `Call` alone. See S1.
- **The shortest of three.** `deadline-ms` is the minimum of the execution's remaining time, a
  `deadline-ms` the caller set, and, for `Call`, the timeout. The scenario
  `deadline_ms_fires_deadline` sets its own 300 ms header on a call with a ten-second timeout, which
  a stamp from the timeout alone would have replaced. Before, a call `.within(&exec)` replaced the
  caller's header whatever it said, so a shorter one was lost; now the shortest wins everywhere. See
  S2.
- **The server.** It already reads `deadline-ms` into the execution's deadline (`dispatch.rs`), as
  for `.within(&exec)`; nothing on the server changes.
- **What pins it.** `a_call_s_timeout_is_the_server_s_deadline` makes a call with a 300 ms timeout to
  the stalling handler through a client link that drops every `cancel` frame (`MutedCancel`, a
  delegating `Link` in the scenario file), so only the deadline the call carried can end the
  handler, and requires it ended with `Deadline` within 300 ms plus the broker's settle budget.
  Without the wrapper the timeout's `cancel` and the deadline race, and a missing stamp would pass
  whenever the `cancel` won. `a_stream_s_timeout_is_per_frame` reads ticks for 2.5 s under a
  one-second timeout and requires every one to arrive, which stamping streams breaks.

### 3. S11 on the links: the hold is fixed when the `open` is sent

- `HoldUntil::at_open(&headers)`, in `ulo_rpc::__private` beside `CANCEL_HOLD`, reads `deadline-ms`
  as the link sends the `open` and records the instant it falls; the entry carries it, and the
  `cancel` that turns the entry into `Cancelled` arms the expiry for `remaining(fallback)`, the time
  left to that instant, or the fallback for an `open` carrying none. The same arithmetic on every
  link, and the parse is the server's, moved onto `CallHeaders`.
- **The fallback.** `CANCEL_HOLD` without the feature; under it the probe's hold, which
  `set_hold` now shortens only for an `open` without a deadline. The existing `hold_runs_out`
  scenario sends its `open` with no headers, so it tests the fallback as before.
- **The new scenario.** `a_held_cancel_lasts_until_its_call_s_deadline` opens with `deadline-ms:
  600`, sets the fallback to 30 s, and requires the entry gone within the scenario's five-second
  patience and no sooner than 600 ms after the `open`. TCP's and smol TCP's three suites and Redis
  declare it not applicable with U17, UDP because it carries no streamed request.
- **Reachability of the fallback.** A `send_stream` call carries its timeout; a `duplex` call
  outside an execution carries no deadline, by decision 2, so a bidi request cancelled before its
  `opened` still reaches the five-second fallback.

### 4. S12 and S9: one feature name, enabled through dev-dependencies

- **The name.** actix's precedent names what the feature is for, `conformance-http2`; these are
  probes, so `conformance-probe`, the same name in `ulo-rpc`, the four links, `ulo-ws` and
  `ulo-ws-hyper`, each manifest saying it is for the conformance suite or tests and not an
  application feature, and each `#[doc(hidden)]` accessor saying the same.
- **Enabled through dev-dependencies.** Each broker link and `ulo-ws-hyper` dev-depends on itself
  with the feature, so `cargo test -p <link> --features integration --test conformance` needs
  nothing new and CI's commands are unchanged. `ulo-rpc-conformance` enables `ulo-rpc`'s feature, as
  `Broker::probe` names `ClientProbe`; the suite is only ever a dev-dependency.
- **What compiles out.** In the links: the `probe` field, the accessor, the `attach` call and the
  check that withholds an `opened`; `fallback_hold` answers `CANCEL_HOLD`. In `ulo-ws`: the count's
  atomic, its `get`, the export and `GatewayDefaults::messages_read`; `MessagesRead` stays a
  crate-private zero-sized type whose `add` does nothing, so the read loop and the tracker keep
  their shape without a `cfg` at each use. See S3.
- **The proof.** `cargo tree -e features,normal` of each of the five crates names no
  `conformance-probe`, and the same read with `dev` names it three times for each link and twice
  for `ulo-ws-hyper`, the control (`tree/`). A scratch crate depending on `ulo-rpc-nats` and
  `ulo-ws-hyper` as an application does, naming `ClientProbe`, `Nats::probe`, `MessagesRead` and
  `Server::messages_read`, fails to compile with four errors at its defaults and compiles with
  `--features probe`, which turns the two features on (`probes/s12-*.txt`).

### 5. S9 of the forty-eighth: both `unsafe` calls avoided

- `Io<T>`'s first read handed the inner read the cursor's uninitialised memory through
  `ReadBufCursor::as_mut`, then `advance`d past what was filled: two `unsafe` calls. It now reads
  into a zeroed stack buffer of `min(buf.remaining(), 8 KiB)`, hyper's initial read buffer, through
  hyper's safe `ReadBuf::new`, and `put_slice`s the bytes into the cursor. The cost is one copy of
  each connection's first bytes, at most 8 KiB, and zeroing that buffer on each poll before the first
  byte arrives; every later read passes through as before.
- `#![forbid(unsafe_code)]` heads `listener.rs`, so the old body fails to compile there.
- **Miri and sanitizers.** No nightly toolchain is installed here (stable, 1.88 and 1.98.1), so
  neither ran, and none was installed. With no `unsafe` left on the path, which the compiler now
  enforces, what remains for a sanitizer to check is hyper's and the listeners'. `rt.rs`'s
  `FuturesIo` keeps its two `unsafe` calls, each with its safety comment; they are not the first-read
  count and are outside this item. See S4.

### 6. S6: F382 in rocket's crate documentation

`ulo-http-rocket`'s documentation has no limits table; the table with a rocket row is the transports
DESIGN's `EmbedLimits` table, which this batch does not edit. The limit is a paragraph after the
crate doc's list of what the adapter declares: WebSocket requires HTTP/1.1 (RFC 6455 §4.2.1), rocket
0.5 keeps no request's version, the adapter hands the app every request as HTTP/1.1, so an HTTP/1.0
upgrade gets a 101 the adapter cannot detect where every other host answers 400. The row is left
for the fold.

### 7. S1: one sentence on each accessor

`AppBuilder::runtime` and `AppHandle::runtime` each say the app's runtime is the core's wrapper
around the one given, not that object, so nothing reaches the given runtime's own type through it,
a downcast included. `Runtime` has no `Any` supertrait, so no downcast exists to fail; the sentence
says what a reader wanting the given object cannot do.

## The tests

- **`ulo-macro-lints`:** a handler taking `Dep<dyn Timer>` and `Dep<dyn Runtime>` on each transport
  (`/clock`, `orders.clock`, `clock`, and `lints.v1.Lints/Clock`, a fifth method on the lint proto),
  an HTTP handler taking `Json<Cow<'static, str>>` and `Query<HashMap<Cow<'static, str>, String>>`,
  and the impl-form constructors of `Named` and `Defaulted` taking the two trait objects.
- **`ulo-ws-hyper`'s `attributes.rs`:** `trait_object_dep_parameters_are_the_app_s_one_runtime`, a
  message handler sleeping a millisecond on `Dep<dyn Timer>` and answering `true` when it and
  `Dep<dyn Runtime>` are one object.
- **`ulo-codegen-tests`' `grpc.rs`:** the same test name over `codegen.v1.Counter/Pause`, a new method
  on `counter.proto`, which sleeps 50 ms on the app's clock and answers `1` for one object; the test
  requires both.
- **The RPC suite, three scenarios,** as decisions 2 and 3 describe:
  `a_call_s_timeout_is_the_server_s_deadline` and `a_stream_s_timeout_is_per_frame` run on every
  link, the second answering by refusal at startup where the link carries no streamed shape, as
  `cancel_mid_stream` does; `a_held_cancel_lasts_until_its_call_s_deadline` runs on the four broker
  links and is declared on the rest.

### Before and after

Each break went through `batch25/scripts/brk.py`, stage e3's script: it replaces each span
(asserting it occurs once), runs the target, writes every touched file and `Cargo.lock` back byte for
byte, and compares a hash over `git diff HEAD` of `crates`, `Cargo.toml`, `Cargo.lock` and `.github`
and every untracked file under `crates`. Every restore reported `ok`, the hash `ea5f7e03..` for the
breaks below. Specs in `batch25/specs/`, output in `batch25/broken/` with `summary.txt`;
`broken/superseded/` keeps the three `from_call` variants of decision 1, hashed `e25e8a90..` on the
tree before the bound replaced the box, the first run of the broker break, whose command did not
parse, and the first run of the `unsafe` break, whose span was cut short and did not compile.

| Break | Target | Result |
| --- | --- | --- |
| `Param<T, ViaContainer>::extract` as an `async fn`, as at `cd5c32dc` | `cargo check -p ulo-macro-lints` | eight errors, "implementation of `std::marker::Send` is not general enough", two at each transport's trait-object handler, none at the constructors |
| `Param<T, ViaCall>::extract` as an `async fn` | same | two errors, "implementation of `Deserialize` is not general enough", at the `Cow` handler |
| the client stamping no `deadline-ms` from a call's timeout | `a_call_s_timeout_is_the_server_s_deadline` on TCP and smol TCP | failed on both, "the stalled handler was still running 1.11s after a call with a 300 ms timeout began" |
| the client stamping the timeout on streams too | TCP's suite | `a_stream_s_timeout_is_per_frame` failed, "the stream failed 1.00s after it opened, its timeout one second per frame: .. the call's deadline passed"; the other 31 passed |
| `HoldUntil::remaining` ignoring the deadline | the held-cancel scenarios on NATS, MQTT, RabbitMQ and Kafka | `a_held_cancel_lasts_until_its_call_s_deadline` failed on each, "the held cancel's entry dropped at its call's deadline did not happen within 5s"; `hold_runs_out` passed |
| `HoldUntil::remaining` dropping at once for an `open` with no deadline | the held-cancel scenarios on NATS | `hold_runs_out` failed at its "dropped before its hold ran out" assertion; the deadline scenario passed |
| the first read handing hyper one byte fewer | `ulo-hyper-serve`'s tests; `ulo-http-hyper`'s suite | the `serve.rs` connection test failed, "the answer did not happen within 5s", and 14 of the suite's scenarios across its two modes failed, the extraction scenario among them |
| the first read counting nothing | `ulo-hyper-serve`'s tests | the `serve.rs` connection test failed at the read count |
| the two `unsafe` calls restored in `listener.rs` | `cargo check -p ulo-hyper-serve` | two errors, "usage of an `unsafe` block", at the restored lines |
| `MessagesRead::add` counting nothing under the feature | `ulo-ws-hyper --test limits` | the stop-reading test failed waiting for its count; the other six passed |
| the restored tree | every target above | every test passed |

## Left for the transports DESIGN fold

Line numbers are those read on `9e01a07f`.

- §5.2, line 943: `deadline-ms` is stamped by the client on every call answered by one reply from its
  timeout, the shortest of that, the calling execution's remaining time and a header the caller set;
  a stream or an event carries only the last two; the server ends the call at it.
- §5.2, line 1091, the client paragraph: `.timeout(d)` on a request or `send_stream` is also the
  call's `deadline-ms`; on `stream` and `duplex` it bounds each frame and is not sent.
- §5.2, the conformance paragraph (line 959) and its list: the three scenarios of decisions 2 and 3,
  and `MutedCancel`.
- Decision 87 (line 1583): a held `cancel` lives until the deadline its call's `open` carried,
  through `HoldUntil`, and `CANCEL_HOLD` is the fallback for an `open` carrying none.
- §4.1 and §4.2, where `MessagesRead` and `Server::messages_read` appear (line 863's load shedding,
  decision 85 at line 1581), and §5.2's `Broker::probe`: each compiles under `conformance-probe`, for
  the conformance tests and not an application feature, enabled through the crate's dev-dependency on
  itself.
- §3.8's `EmbedLimits` table, the `upgrades` row (line 706): rocket hands the app every request as
  HTTP/1.1, so an HTTP/1.0 upgrade is answered 101 (F382).
- §3.7's `Io<T>` (line 590) and decision 93 (line 1589): the first read copies into a buffer of its
  own through `ReadBuf::new` and `put_slice`, no `unsafe` left; `listener.rs` forbids it.
- §8, "The app" (line 1292), and decision 89 (line 1585): the app's runtime is not the object given,
  and nothing reaches its type through it.
- §2.2 or §2.3, wherever the generated call's `Param` is described: `extract` declares `+ Send` in
  each impl's signature, which is what lets a parameter type carrying a lifetime compile (F384,
  F387).

## Needs sign-off

Numbered from S1 for this batch.

### S1. Only a call answered by one reply stamps its timeout

Decision 2. A stream's timeout bounds each frame's wait and an event's bounds the publish, so neither
is a moment the client abandons the server's work; stamping them would end long streams at the
per-frame timeout. The alternative is the response's literal reading, every call, which would also
need a separate per-call deadline knob for streams that run longer than their frame timeout.

### S2. The stamp is the shortest of three, a caller's header included

Decision 2. A caller's own `deadline-ms` is kept when shortest, and replaced when the timeout or the
execution is shorter. Before, `.within(&exec)` replaced it whatever its value. The alternative is
the caller's header always replaced, which would break `deadline_ms_fires_deadline`.

### S3. `MessagesRead` stays as a private zero-sized type without the feature

Decision 4. The read loop and the tracker keep one shape, and the type, the count and both accessors
are absent from a default build's API. The alternative is a `cfg` on the tracker's field, its
constructor's parameter and the read loop's call.

### S4. The first-read path ran under neither Miri nor a sanitizer

Decision 5. Both need a nightly toolchain, which is not installed, and the batch did not install
one; the path has no `unsafe` left, which `forbid` holds. The alternative is installing nightly and
Miri to run the `serve.rs` tests, which hyper's socket I/O would likely keep out of Miri anyway.

### S5. The RPC suite's waiting handlers take the clock as a parameter

Decision 1. Six handlers each name `timer: Dep<dyn Timer>`; the alternative is keeping stage e3's
controller fields, one per controller, now that the parameter compiles.

## Verification

Full, unfiltered output of every run is in the session scratchpad, `batch25/`: `probes/` for
F384's, F387's and S12's, `broken/` (with `summary.txt` and `superseded/`) and `specs/` for the
breaks, `runs/` for the runs during the build, `suites/` for the broker suites (with
`brokers-summary.txt`), `tree/`, `verify/` and `scripts/`. The final tree's hash, by `brk.py`'s
measure, is `b21a8831..` (`verify/final-tree-hash.txt`); it differs from `ea5f7e03..`, on which the
breaks, the workspace checks and the workspace test ran, by the two `#[allow(clippy::manual_async_fn)]`
attributes and their comment line in `ulo-transport`'s `__private.rs`, compiled since by the
clippy runs below.

- **The local RPC suites, once each, in the workspace run** (`verify/workspace-test.txt`): TCP, TCP
  CBOR and TCP TLS 32 passed and 3 ignored each; the smol TCP link's three 32 and 3 each; UDP 29 and
  6. Each gained the two deadline scenarios as passes and the held-deadline scenario as a
  declaration; UDP passes the stream scenario by its refusal at startup. An earlier run of the same
  targets, before the stream scenario existed, gave 31 and 3, and 28 and 6 (`runs/rpc-local-1.txt`).
- **The broker suites, once each,** one crate at a time, `--features integration --test conformance
  --locked` with the OpenSSL flags, libtest's time (`suites/brokers-summary.txt`, `late-summary.txt`
  in `verify/`): NATS 34 passed and 1 ignored (4.28 s); Redis 33 and 3 (6.32 s); MQTT 39 and 1
  (7.24 s); RabbitMQ with `ULO_CONFORMANCE_PARALLEL=6` 34 and 1 (85.86 s); Kafka 35 and 1 (35.61 s).
  Each is batch 24's count plus the three scenarios, run or declared. NATS's first run, 33 passed,
  compiled the suite before the stream scenario was added and is superseded by the second
  (`suites/broker-nats-{1,2}.txt`). RabbitMQ took 85.86 s where stage e3 recorded 30.60 s; it was run
  once, and the cause was not traced.
- **`ulo-ws-hyper` and the WebSocket suite hosts,** in the workspace run: `ulo-ws-hyper` 108 passed
  and 1 ignored, `attributes.rs` 12 with the new test, `limits.rs` 7 through the crate's
  dev-dependency on itself, the suite 31 on tokio and 31 on smol; `ulo-ws`'s hand-off, pipe and smol
  hosts 31 each; the embed hosts 31 on axum, salvo and poem, 29 and 2 on rocket, 1 and 30 on actix.
- **The HTTP adapters,** in the workspace run: the HTTP suite 38 on axum and poem, 39 on salvo, 36
  and 2 on rocket, 34 and 4 on actix, 36 and 2 on the hyper backend on tokio and on smol, `/endless`
  taking its clock as a parameter in each.
- **`ulo-codegen-tests`,** in the workspace run: 54 passed, `grpc.rs` 7 with the new test,
  `compile_fail.rs` 1.
- `cargo +1.98.1 clippy -p ulo-macro-lints --all-targets --no-deps --locked -- -D warnings`: exit 0,
  on the final tree (`verify/clippy-lints-deny-2.txt`) and before the two attributes
  (`clippy-lints-deny.txt`).
- `cargo check --workspace --all-targets --all-features` with
  `CFLAGS=-I/opt/homebrew/opt/openssl/include LDFLAGS=-L/opt/homebrew/opt/openssl/lib`, and with
  defaults: exit 0, the 17 known warnings in `crates/ulo/src` and no other location
  (`verify/check-*.txt`, `scripts/diag.py`, which reported a planted location in its self-test,
  `verify/diag-selftest.txt`).
- `cargo +1.88 check --workspace --all-targets --exclude ulo-http-salvo --exclude
  ulo-graphql-async-graphql`: exit 0, the same 17 and no other (`verify/check-1.88.txt`).
- `cargo test --workspace --no-fail-fast` with the OpenSSL flags: 1,063 passed, 0 failed, 138
  ignored across 195 test results (`verify/workspace-test.txt`). Stage e3's 1,047 plus 16: two on
  each of TCP's and the smol link's six suites, two on UDP, and the WebSocket and gRPC tests; ignored
  rises by seven, the held-deadline declaration on those seven suites.
- **The tree checks,** `cargo tree -p <crate> -e normal -i tokio --locked` (`tree/summary.txt`):
  stdout is empty for `ulo`, `ulo-transport`, `ulo-net`, `ulo-http`, `ulo-rpc`, `ulo-ws`,
  `ulo-graphql-ws`, `ulo-graphql-http`, `ulo-ws-conformance`, `ulo-runtime-conformance`,
  `ulo-smol`, `ulo-rpc-tcp-core`, `ulo-rpc-tcp-smol` and `ulo-rpc-conformance` at its defaults, and
  for `ulo-rpc` and `ulo-ws` with `conformance-probe`, each "did not match any packages" or "nothing
  to print"; `ulo-ws-hyper` and `ulo-rpc-conformance` with `tokio` print tokio, the controls. The
  feature reads are decision 4's.
- `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --lib -p <crate>` for `ulo`, `ulo-transport`,
  `ulo-rpc`, `ulo-rpc-conformance`, the four broker links, `ulo-ws`, `ulo-ws-hyper`,
  `ulo-hyper-serve`, `ulo-http-conformance`, `ulo-http-rocket` and `ulo-macro-lints`, and for
  `ulo-rpc`, `ulo-rpc-nats`, `ulo-ws` and `ulo-ws-hyper` with `conformance-probe`: each exits 0
  (`verify/doc-*.txt`).
- `cargo +1.98.1 clippy` over the eighteen touched crates with `--all-targets --all-features
  --no-deps --locked` and the OpenSSL flags: exit 0, 22 warning locations outside `crates/ulo/src`,
  two of them on changed lines, `manual_async_fn` at the two `extract`s, which the attributes answer
  (decision 1); a rerun over `ulo-transport` reports no location on a changed line
  (`verify/clippy-touched.txt`, `clippy-transport-2.txt`, `scripts/changed_lines.py`, whose first
  run found those two).
- **Containers:** the user's four, seaweedfs, mailpit, postgres:18 and redis:7, before the broker
  suites, after each, and at the end (`runs/docker-before.txt`, `suites/brokers-summary.txt`,
  `verify/docker-after.txt`). Every container a suite started was removed by it. No permission check
  refused an action.
- **Miri and sanitizers:** not run; decision 5 and S4.
