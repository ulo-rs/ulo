# Build plan: race 3

Race 3 builds the integration layer on the race 2b tree: configuration, health over HTTP and gRPC,
the shared database policy with five stores and their conformance suite, the CLI's templates, and
W3C trace propagation with OpenTelemetry export. The SPI extensions it needs are I1–I16
(`integrations/DESIGN.md` §9); I12, I13 and I14 are the fourth response's P2, H5 and P9, and its P6
widens I3 by the secret collector. I15 and I16 are the fifth response's S5, which also widens I5 by
the header text and `TraceOrigin`; its S4 widens I14 to a stored copy every write goes through.

The spine comes first, as in 2a and 2b, and runs as stage 0: three sub-agents in parallel, on
disjoint files. Between them they write every core and transport extension as finished code, the
insertion points in the four hubs, the move of the RPC suite's shared pieces into a crate of their
own, the workspace manifest, and every new crate's module tree and public signatures with `todo!()`
bodies. The areas then fill the skeletons stage by stage, in the order of `integrations/DESIGN.md`
§8, each building against the signatures the spine froze.

`integrations/DESIGN.md` is the spec, over `DESIGN.md` and `transports/DESIGN.md`, and carries the
fourth and fifth responses as ruled. `integrations/RESPONSE.md` breaks a tie, the latest response
winning: the fifth response, signed off 2026-10-19, then the fourth, signed off the same day (the
table below lists where each decides). `divergences/race3-S.md` lists every place the spine departs
from the designs or fills what they leave open, each entry naming the sub-agent that wrote it; those
entries await the user's sign-off like any area's.

## Names

The designs write `fw` for the crate prefix; the workspace writes `ulo`. Where a design name and a
workspace name differ by more than the prefix:

| Design | Workspace |
|---|---|
| `ulo-db-mongo` | `ulo-db-mongodb`, the directory on disk (fourth response H6) |
| "a shared `publish = false` crate both suites depend on" (I11) | `ulo-conformance-support` (new; the name is the spine's, logged) |
| E1, E2, E3, E4 (second response) | I3, I5, I7, I2 |
| `fw::spawn_with`, `fw::Permanent`, `fw::is_permanent` | `ulo::spawn_with`, `ulo::Permanent`, `ulo::is_permanent` |

Every other crate is the design name with the prefix swapped.

## Where the fourth and fifth responses decide

Each row is a point a response decided. `integrations/DESIGN.md` states each as ruled; the area
named builds it. Where a fifth-response row and a fourth-response row cover one subject, the
fifth's governs, and the fourth's row below states only what still holds of it.

| Point | Ruling | Area |
|---|---|---|
| P1 | `ulo-db` stays at 1.88 and names no lock primitive; `File::lock` sits in the store crates that speak SQLite | D1, D2, S2b |
| P2 | a missing key whose full type name equals a bound key's with a different `TypeId` gets the help "the same type from two versions of its crate; `cargo tree -d` lists them" | S-core |
| P3 | SeaORM's store validates with SQLx's option parser; the whole URL and its password are both registered with `m.secret(..)`; the options' `Debug` writes the URL redacted; `validate` checks the scheme itself, shared with `ulo-db-sqlx` | S2a, D2 |
| P4 | YAML on `saphyr`'s `MarkedYaml` into the origin tree, behind `yaml`, after TOML and JSON; aliases bounded (R2), merge keys refused naming the line, scalars by the YAML 1.2 core schema | C |
| P5 | the crate's own `.env` parser, the grammar table of P5, a key twice in one file a problem naming both lines | C |
| P6 | a scoped collector `Secret::deserialize` writes to, installed by `ulo-config`, beside the name-based path; a problem under a secret's path rendered from path and kind alone | S-core (collector), C |
| P7 | at most one leaf-type error per section, a stated limit | C |
| P8 | `ulo-otel-otlp` ships a `ureq` client by default and `reqwest-blocking` as an opt-in feature; runtime-free at default features | O3, S-skel (levels) |
| P9 | tonic-health's `NOT_FOUND` documented; no writer writes an unchanged status (S4) | H2, S-core |
| P10 | `.lag` guidance: zero with a `preStop` sleep, a few seconds without one; the default stays zero | H1 |
| H1 | `Store` gains `const CREATE_CONNECTS: bool`; for such a store `ulo-db` loops around `create`, stopping on `Permanent`, under the bounds S1 gives; Redis sets `set_number_of_retries(0)`; Diesel offers `.lazy()` | D1, S2b, S2d |
| H2 | no change to `Runtime`; Diesel's blocking path as R4 gives it | S2b |
| H3 | `ulo-db` owns the key, the order, the bound and the error; each store the dedicated connection and the primitive: `pg_advisory_lock(K)`, `GET_LOCK('ulo-db.migrate.' + database, timeout)` with a hash past 64 characters, an OS lock on `<file>.ulo-migrate.lock`; `K` is the first eight bytes of SHA-256 of `"ulo-db/migrations"` as a big-endian `i64` | D1, D2, S2a, S2b |
| H4 | an invalid incoming `traceparent` in pass-through gives no context; in pass-through both headers are forwarded exactly as received and a caller's `tracestate` on a forwarding call is refused `reserved_header`, its message naming that a vendor entry needs an exporter; Level 1, flags forwarded unchanged in pass-through; which context merges is S5's `TraceOrigin` | S-core (reservation), O1 |
| H5 | `TraceSource` is a core hook called inside `span::call` between creating the span and first entering it: `TraceSource::start(&span, inbound)` answers the execution's own context, written through `Execution::set_trace`, once, by the holder, before `dispatch`; each transport extracts before `Execution::open` and passes it in `ExecOptions::trace` | S-core, O1, O2 |
| H6 | `ulo-db-mongodb` keeps its name; no `mysql-rsa`; `mongodb+srv` validated by scheme and host with the SRV lookup in `create`; the TLS feature as R1 gives it, Redis's preference as S3 | D2, S2c, S2d |
| R1 | `ulo-db-sqlx`'s TLS is SQLx's `tls-rustls-ring-native-roots`: ring, the workspace's provider, with the platform's roots; rumqttc's aws-lc-rs is MQTT's limit, documented | D2 |
| R2 | YAML aliases bounded by a pre-scan of `saphyr-parser`'s events before the loader runs: the nodes inside each anchored node counted as its events arrive, the count added at every alias, refused past 100,000 nodes with a problem naming the alias's line, a fixed limit; if the events lack anchor IDs or positions, every alias refused naming its line | C |
| R3 | `ulo-config`'s error type renders every type and value problem from the kind of the value found, never the value, buffered content included; a leaf type's own `Error::custom` message is printed as written, the limit stated on `Secret`'s `Deserialize` docs and `ulo-config`'s | S-core (`Secret`'s docs), C |
| R4 | `ulo-db-diesel`'s `Given`, a `SpawnBlocking` over the given runtime's handle captured once in `create`; each SQLite connection built with `with_runtime(conn, Given(handle.clone()))`; `Given::spawn_blocking` calls `handle.spawn_blocking(..)`; the migrations run through the same `Given` | S2b |
| R5 | SQLx's `runtime-smol` beside `runtime-tokio` is decided by a stage-2 test: a `PoolConnection` dropped on a thread with no tokio context, asserting no task ran on smol's executor; a failure returns as an S-item | the coordinator (stage-2 test) |
| S1 | one bound, `connect_within`, and one retry loop per store: `.ready(..)` with its retries, backoff and `.timeout(connect_within)` for a store whose `create` connects nothing; for one whose `create` connects, the `create` loop under `connect_within`, the construction bound set to `connect_within` through `.timeout(..)` on the binding handle, then `.ready(..)` as one confirming ping, `.retries(0)`, under the default attempt bound | D1, S2c, S2d |
| S2 | `CREATE_CONNECTS` is `true` for the whole MongoDB store, a `const`; MongoDB therefore has no `.lazy()` | S2c |
| S3 | `prefer_tokio()` once per process, under a `OnceLock` in `ulo-db-redis`; a refusal read for the current preference where redis-rs allows (tokio continues, anything else a `Permanent` refusal at `connect`), a `warn` naming the conflict where it does not | S2d |
| S4 | `GrpcHealth` is the only path to the reporter and does not expose it; every write, the server's, the bridge's and the application's `set` and `set_named`, compares with its stored copy before reaching the reporter | S-core, H2 |
| S5 | `TraceContext` keeps both headers' text as received beside the parsed fields, a forward writing the text; `TraceOrigin::{Received, Originated}` inside it, set by `ExecOptions::trace` and `set_trace`, read by the client's `tracestate` rule; `App` resolves `Option<Dep<dyn TraceSource>>` once at `connect`, read through `AppHandle::trace_source()`; `ExecOptions::span(tracing::Span)` makes `execute` call `TraceSource::start` and `set_trace`; `Execution::open` unchanged | S-core, O1, O2 |
| N1 | `ulo-db-diesel` states 1.89, for `File::lock` | S2b |
| N2 | `ureq` pinned exactly, `=3.4.2` | S-skel (the workspace entry), O3 |
| N3 | `K = 3490428366386280370` | D1 |

## Rules

The rules of `BUILD_PLAN_2B.md` apply, restated with this race's names, with one exception (9).

1. **No `cargo` in any form during the race**, and no git command. Each area reads its own files
   for type and borrow errors before handing them back. A library fact an area needs and cannot
   read from the registry sources under `~/.cargo/registry/src` is logged as "Not verified".
2. **No tests and no examples.** `ulo-db-conformance` and `ulo-conformance-support` are library
   crates and are written in the race. Every `tests/` file, the per-store `tests/conformance.rs`
   included, is the coordinator's after the first compile.
3. **Own files only.** An area edits the files its table assigns and no others. A change needed in
   another area's file, a new `pub(crate)` item or a changed signature included, is a request in
   `divergences/race3-<area>.md` naming the file, the item and the reason; the coordinator routes
   it to the owner. The root `Cargo.toml`, `runtime-levels.txt`, `.github/workflows/ci.yml` and
   `scripts/` belong to the coordinator after the spine: a new workspace dependency, a level entry
   or a CI job is a request too. An area owns its crates' `Cargo.toml` files.
4. **The contracts below are frozen.** An item listed under "Cross-area contracts" keeps its name,
   fields and signature. Its owner may add private helpers and fields no other area reads.
5. **Public signatures are frozen.** A public item keeps the signature the spine gives it.
   Adding, renaming or reshaping one is a divergence.
6. **Every divergence is logged** in `divergences/race3-<area>.md`, one entry each: what the design
   says (or that it is silent), what was written, and why. A behaviour the design states and the
   code does differently is a divergence even when no signature changes. The log also records what
   the area replaced on `master` and what it did not carry over.
7. **Code style.** Rust 2024, stable, each crate at the floor the table under S gives. The core
   depends on no async runtime, and `ulo-db` names no driver. Comments say what the code cannot:
   why, ordering constraints, invariants; public docs say when to use an item and what the design
   states about it. No labels, no step narration, no section dividers.
8. **`todo!()` is the only panic the spine leaves**, and every one is replaced. The core and the
   integrations never panic on a path the design gives a typed error.
9. **The race runs in stages** (the exception). The spine runs first, its three sub-agents in
   parallel on disjoint files, and holds every core extension of all five stages; the areas then run in the order of `integrations/DESIGN.md` §8,
   the areas of one stage in parallel, a stage starting once every area of the stage before has
   handed back its files and its log. An area builds against the spine's signatures; where it reads
   an earlier stage's bodies and disagrees with them, it logs the disagreement and edits nothing.

   | Stage | Areas | DESIGN §8 |
   |---|---|---|
   | 0 | S-core, S-move, S-skel, in parallel | the core and transport extensions of every stage: I1, I2, I3 with P6's collector, I4, I5, I7, I8's reservation, I12, I13, I14, I15, I16 (S-core); I11 (S-move); I10 and the skeletons (S-skel) |
   | 1 | C, H1 | 1: configuration and `ulo-health` with HTTP |
   | 2 | D1, D2, D3 | 2: `ulo-db` over `.ready`, SQLx and the suite |
   | 3 | S2a, S2b, S2c, S2d, H2 | 3: the other stores; `ulo-health-grpc` |
   | 4 | L | 4: the CLI's templates |
   | 5 | O1, O2, O3 | 5: the carriers in every hub and the client (I6, I9), `ulo-otel`, `ulo-otel-otlp` |

10. **Models.** Sonnet builds and is the default. Opus takes an area whose work turns on
    concurrency, lifecycle, ordering or timing. Fable runs only on the user's approval, and no area
    here asks for it. Each area states its model on a `Model:` line.

## S. The race-3 spine (stage 0)

The spine owns the core extensions, the transport changes that are not carriers, the insertion
points of the carriers, the I11 move, the workspace manifest and the level list, and every new
crate's skeleton. It runs as three sub-agents in parallel, S-core, S-move and S-skel, and no file
belongs to two of them. Each logs into `divergences/race3-S.md` under its own name. A change one
needs in another's file is a request in that log, routed by the coordinator after stage 0, as rule 3
has it between areas; S-core and S-move need nothing from the root `Cargo.toml` or
`runtime-levels.txt` beyond what S-skel writes from this plan, and a new workspace dependency either
finds it needs is such a request.

S-skel does not wait on S-core: it codes against `integrations/DESIGN.md` §9, the cross-area
contracts below and the surface this plan fixes for S-core (below), so every skeleton signature
matches S-core's real types. S-core may add private items and accessors no skeleton names; it may
not rename, reshape or drop an item of the fixed surface.

### S-core: the core and transport extensions

Model: Opus. The readiness loop's early end, `Permanent` through redaction, the `TraceContext` and
`TraceSource` core change with its resolution at `connect`, `ExecOptions::trace` and `::span`,
`span::call`, the RPC client's reservation and `GrpcHealth`'s stored copy are lifecycle and ordering
work.

#### Finished code

| Files | Holds |
|---|---|
| `crates/ulo/src/runtime.rs` | I1: `fn name(&self) -> &'static str` on `Spawn` (`:24`), defaulted to `"unknown"`; `impl Spawn for Clocked` (`:68`) answers its `spawner`'s name, so `.runtime(r).timer(t)` still answers `r`'s |
| `crates/ulo-runtime-tokio/src/lib.rs`, `crates/ulo-runtime-smol/src/lib.rs` | I1: `name()` answering `"tokio"` in `impl ulo::Spawn for Tokio` (`:122`) and `"smol"` in `impl ulo::Spawn for Smol` (`:192`); the two test runtimes in `crates/ulo-rpc/src/client.rs` (`:1536`, `:1712`) keep the default |
| `crates/ulo/src/error/mod.rs` | I2: `#[non_exhaustive] pub struct Permanent(BoxError)`, `Permanent::new(e)`, `Display` the inner error's, `source()` the inner error; `is_permanent(&StartupError) -> bool` beside `is_panic` (`:583`), walking the chain under the same depth bound and recognising `Permanent` inside a `Redacted` as one of the core's own types |
| `crates/ulo/src/lifecycle/connect.rs` | I2: in `run_readiness` (`:83`), the `Outcome::Done(Err(e))` arm (`:102`) reads the attempt's error for a `Permanent` before `construct_error` redacts it, and a `Permanent` ends the check at once as `FailureReason::Errored(..)`, with no further attempt and no backoff |
| `crates/ulo/Cargo.toml`, `crates/ulo/src/redact.rs`, `crates/ulo/src/__private.rs` | I3: the `serde` feature over the workspace's `serde`, optional; `impl<'de, T: Deserialize<'de>> Deserialize<'de> for Secret<T>` (`Secret` at `redact.rs:69`) through `deserialize_newtype_struct("Secret", ..)`, and no `Serialize`; its documentation states R3's limit, that under `#[serde(flatten)]` or `#[serde(untagged)]` the secret is registered but an `Error::custom` message its inner type raises is printed as written; P6: the scoped collector `Secret::deserialize` records into, sound for one synchronous deserialisation, installed by `ulo-config` through the doc-hidden entry the fixed surface names |
| `crates/ulo/src/graph/visibility.rs`, `crates/ulo/src/error/wiring.rs` | I12 (P2): `near_spelling` (`visibility.rs:319`, `spelled_alike` at `:357`) also matches a visible key whose full type name equals the missing key's while the `TypeId`s differ, and the rendering of `WiringError::Missing` (`wiring.rs:178`, rendered at `:579`) gives that match the help "the same type from two versions of its crate; `cargo tree -d` lists them" |
| `crates/ulo/src/execution/trace.rs` (new) | I5: `TraceContext` and `TraceState`, runtime-free, no clock and no randomness, with the W3C Level 1 rules PROBES_obs.md §5 lists: the 55-character `00` grammar in lowercase hex, a higher version parsed by the `00` offsets with end-or-dash after the flags, `ff` and all-zero IDs invalid, a duplicated `traceparent` a restart, whitespace trimmed, flags as bits with unknown bits zeroed on output, `tracestate` parsed only beside a valid `traceparent`, at most 32 members, the key and value grammar, an invalid member discarding the list, `with_entry` adding at the left and truncating to 512 characters by whole entries. A parsed context keeps both headers' text as received beside the parsed fields, and a `Received` context is written out as that text; an `Originated` one is written as version `00` (S5). `TraceOrigin::{Received, Originated}` inside `TraceContext`. `TraceMode::{On, Off}`, `On` the default. I7: `TraceSource`, `start(&Span, inbound)` answering the execution's own context (H5) |
| `crates/ulo/src/execution/mod.rs` | I5: `ExecOptions::trace(TraceContext)` beside `deadline` (`:66`), marking the context `Received`, its field in `ExecOptions` (`:55`), `deadline` rebuilt so the options compose; I16: `ExecOptions::span(tracing::Span)` and its field; the inbound context and the set-once own context on `ExecShared` (`:107`) beside `deadline` (`:122`); `Execution::trace()` beside `deadline()` (`:205`) and the same on `ExecutionRef` (`:287`), answering the own context once set and the inbound one before; I13: `Execution::set_trace(TraceContext)`, taking effect once, marking the context `Originated`, on `Execution` and not `ExecutionRef`, before `dispatch`, as `route_to` (`:172`) does; `Execution::open` (`:145`) reads no span; `mod trace` |
| `crates/ulo/src/app/shared.rs` | I5: the inbound context copied from `opts` beside `deadline: opts.deadline` (`:172`); I15: the resolved source on `AppShared` (`:31`), set once at `connect`; I16: `AppShared::execute` (`:186`) takes the span out of `opts` before `open` and, with a span and a resolved source, calls `TraceSource::start(&span, inbound)` and `set_trace` before the closure runs |
| `crates/ulo/src/app/mod.rs` | I5: `AppBuilder::trace(TraceMode)` (`AppBuilder` at `:84`), the mode on `AppConfig` (`:92`); I15: `App::connect` (`:259`) resolves `Option<Dep<dyn TraceSource>>` after `connect::connect` (`:265`) has built every singleton, through `Dep::into_arc` (`crates/ulo/src/dependency/dep.rs:27`) |
| `crates/ulo/src/app/handle.rs` | I15: `AppHandle::trace_source() -> Option<&Arc<dyn TraceSource>>` beside `timer()` (`:106`), a plain accessor |
| `crates/ulo/src/testing.rs` | `TestApp::trace(TraceMode)` if S-core answers §14's question with yes; untouched otherwise |
| `crates/ulo/src/lib.rs` | re-exports: `Permanent`, `is_permanent`, `TraceContext`, `TraceState`, `TraceOrigin`, `TraceMode`, `TraceSource` |
| `crates/ulo-transport/src/span.rs` | I7: `call` (`:35`) declares `otel.kind` and `otel.status_code` empty beside its other fields, with their constants, for each hub to record, and calls the source `AppHandle::trace_source()` answers with the new span and the execution's inbound context before returning it, writing the answer through `Execution::set_trace`; with no source the inbound context stays as it is (H5). The call sites change with the signature, in the hub files below |
| `crates/ulo-grpc/src/health.rs`, `crates/ulo-grpc/src/server.rs` | I4: `GrpcHealth::delegate_overall()`, after which `drain` (`server.rs:406`) writes no status for `""`; I14 (S4): `GrpcHealth` (`health.rs:16`) keeps the last status written per name beside its `reporter` (`:17`), shared across clones and starting with `""` at `SERVING`, and every write, `set` (`:37`), `set_named` (`:42`) and the server's at `bind` (`server.rs:385-387`) and `drain` (`:406-409`), compares with that copy and reaches the reporter only on a change, the comparison and the write under one lock. `HealthReporter` stays `pub(crate)`. tonic-health 0.14.6's reporter has no read (its methods are `new`, `set_serving`, `set_not_serving`, `set_service_status`, `clear_service_status`; read in the registry) |
| `crates/ulo-rpc/src/client.rs` (also in the next table), `crates/ulo-rpc/src/transport.rs` | I8 as H4 and S5 give it: `Spec::reserved` (`client.rs:536`, called at `:951`, `:1009`, `:1115`) refuses a caller's `traceparent`, `BadRequest` with `reason: "reserved_header"` at the await, exactly on a call whose `.within` execution carries a context, and a caller's `tracestate` on such a call when the context's `TraceOrigin` is `Received`; a call with no `.within`, or within an execution with no context, keeps both as ordinary headers. `CallHeaders`'s doc (`transport.rs:208`, the `traceparent` sentence at `:212`) states both rules where it names `deadline-ms` alone today. The F400 unit test (`client.rs:1587-1597`) sends `traceparent` with no `.within` and still holds; S-core checks it by reading |

#### Insertion points, filled by O1

S-core writes each call and the helper it calls, `pub(crate)` in a new `trace.rs` per hub with a
`todo!()` body, and the `mod trace;` line. O1 owns these files from stage 5.

| Files | Holds |
|---|---|
| `crates/ulo-http/src/service.rs`, `src/trace.rs` (new), `src/lib.rs` | I9: extraction from `req.head.headers` before `Execution::open` (`service.rs:160`), passed in `ExecOptions::trace`; `span::call` at `:176` given what the hook needs; the embedding reaches it through `AppService` |
| `crates/ulo-grpc/src/dispatch.rs`, `src/client.rs`, `src/trace.rs` (new), `src/lib.rs` | I9: extraction from the request's headers into the `opts` built at `dispatch.rs:154-157`, before `Execution::open` (`:158`) and before `req.into_parts()` (`:160`) consumes them; `call_span` (`:508`, called at `:169`) given what the hook needs. I6: injection into the request's metadata in `outgoing` (`client.rs:120`) ahead of its early return for an execution with no deadline (`:121`) |
| `crates/ulo-rpc/src/dispatch.rs`, `src/client.rs`, `src/trace.rs` (new), `src/lib.rs` | I9: extraction from `incoming.headers` beside the `deadline-ms` read (`dispatch.rs:192`), into the `opts` built at `:220-223` for `Execution::open` (`:224`), and on the unhandled path (`:278`); `call_span` (`:660`, its `span::call` at `:662`, called at `:239` and `:289`) given what the hook needs. I6: injection in `written` (`client.rs:593`), its early return for a call with no bound moved behind the injection, on both call sites (`:556` for a request or stream, `:1020` for an event) |
| `crates/ulo-ws/src/connection.rs`, `src/gateway.rs`, `src/trace.rs` (new), `src/lib.rs` | I9: extraction from the handshake's headers (`handshake`, `connection.rs:582`) into the connect phase's `Execution::open` (`:798`), its span at `:838`; the connect execution's own context kept on the connection beside the inputs `Connection::seed` (`:175`) seeds, and passed as the parent of each message's execution (`:1391`, its span at `:1401`), a hand-written gateway's (`:143`, `gateway.rs:576`) and the terminal one (`:1527`, `open_terminal` at `:1531`) |

S-core writes 36 files: 22 of finished code and 14 at the insertion points, the RPC client counted
once.

#### The surface S-core's skeleton-facing items take

Fixed by this plan. S-skel names these in signatures and manifests; S-core writes them as given.

| Item | Fixed as |
|---|---|
| `ulo::Spawn::name` | `fn name(&self) -> &'static str`, default `"unknown"` |
| `ulo::Permanent` | `#[non_exhaustive] pub struct Permanent(BoxError)`, the field private; `Permanent::new(e: impl Into<BoxError>) -> Permanent`; `Debug`, `Display` the inner error's, `std::error::Error` with `source()` the inner error |
| `ulo::is_permanent` | `pub fn is_permanent(e: &StartupError) -> bool` |
| the `serde` feature | named `serde` on `ulo`, off by default; `impl<'de, T: Deserialize<'de>> Deserialize<'de> for Secret<T>` behind it |
| the collector entry | `ulo::__private::collect_secrets<R>(f: impl FnOnce() -> R) -> (R, ulo::__private::CollectedSecrets)`; `CollectedSecrets::register(self, m: &mut ulo::ModuleDef<'_>)`, which calls `m.secret(..)` for each; doc-hidden |
| `ulo::TraceContext`, `ulo::TraceState` | `pub struct`s, fields private, defined in `crates/ulo/src/execution/trace.rs` and re-exported at the crate root; `Clone`, `Debug`, `PartialEq`, `Eq` |
| `ulo::TraceOrigin` | `pub enum TraceOrigin { Received, Originated }`, `Clone`, `Copy`, `Debug`, `PartialEq`, `Eq`; read by `TraceContext::origin(&self) -> TraceOrigin` |
| `ulo::TraceMode` | `pub enum TraceMode { On, Off }`, `Default` `On`, `Clone`, `Copy`, `Debug`, `PartialEq`, `Eq` |
| `ulo::TraceSource` | `pub trait TraceSource: Send + Sync + 'static { fn start(&self, span: &tracing::Span, inbound: Option<&TraceContext>) -> TraceContext; }`, dyn-compatible |
| `ExecOptions` | `pub fn trace(self, ctx: TraceContext) -> Self`, `pub fn span(self, span: tracing::Span) -> Self`, beside `deadline` |
| `Execution`, `ExecutionRef` | `pub fn trace(&self) -> Option<&TraceContext>` on both; `pub fn set_trace(&self, ctx: TraceContext)` on `Execution` alone |
| `AppBuilder` | `pub fn trace(self, mode: TraceMode) -> Self` |
| `AppHandle` | `pub fn trace_source(&self) -> Option<&Arc<dyn TraceSource>>` |
| `ulo_grpc::GrpcHealth` | `pub fn module() -> DynamicModule`, `pub fn delegate_overall(&self)`, `pub async fn set<S: NamedService>(&self, status: ServingStatus)`, `pub async fn set_named(&self, name: &str, status: ServingStatus)`; no public field and no accessor to the reporter |

`span::call`'s signature, the hubs' `trace.rs` helpers and `TraceContext`'s methods other than
`origin` are S-core's to choose: only S-core's own files and the stage-5 areas, which build after
it, name them.

Design sections: integrations DESIGN §1 (the change table), §2.3, §3.4, §6.1, §6.3, §6.4, §9 (I1–I9,
I12–I16), §13.3, §13.4; fourth response P2, P6, P9, H4, H5; fifth response R3, S4, S5; PROBES_obs.md
§1, §2, §5, §7; PROBES_config.md §6; DESIGN §9.3, §10.1 step 3, §10.2; transports DESIGN §2.9,
decision 124.

### S-move: the shared suite crate (I11)

Model: Sonnet. A move, not a rewrite: items keep their names, bodies and docs, and the RPC suite
reads them at their old paths.

| Files | Holds |
|---|---|
| `crates/ulo-conformance-support/Cargo.toml`, `src/lib.rs`, `src/failures.rs`, `src/relay.rs`, `src/tokio_harness.rs`, `src/smol_harness.rs` (new; `publish = false`; features `tokio` and `smol` as the RPC suite has them) | moved from `crates/ulo-rpc-conformance/src/`, not rewritten: `report` (`lib.rs:88`), `startup_failed!` (`:107`) with `failures_dir` (`failures.rs:13`), `Harness` with `block_on_one_thread` (`:132`), `Budget` with `stall` (`:221`), `PARALLEL_VAR` and the slot bound of `__private`, the `not_applicable` stamping rules, `relay::{Relay, Outage, shadowed, unshadowed, reachable}` (`relay.rs:243`, `:267`), `OnTokio` and `OnSmol`. `failures_dir` still finds the workspace root from the manifest path, now this crate's |
| `crates/ulo-rpc-conformance/Cargo.toml`, `src/lib.rs`; `src/failures.rs`, `src/relay.rs`, `src/tokio_harness.rs`, `src/smol_harness.rs` (removed) | the RPC suite depends on the support crate and forwards its `tokio` and `smol` features, and re-exports every moved item at its old path, so `src/cases/*.rs`, `src/rt.rs` and every link's `tests/conformance.rs` read them unchanged; `Broker`, `conformance_suite!`, `Stamped` and `ClientProbe`'s use stay |

S-move writes 12 files: 6 new, 2 edited, 4 removed. The workspace member line and the two level
entries for `ulo-conformance-support` are S-skel's.

Design sections: integrations DESIGN §4.9, §9 (I11); transports DESIGN §3.8, §5.2.

### S-skel: the workspace and the skeletons

Model: Sonnet. Manifests and signatures, each fixed by the area tables, DESIGN §9, the cross-area
contracts and S-core's fixed surface; every body `todo!()`.

| Files | Holds |
|---|---|
| `Cargo.toml` | `members` gains `crates/ulo-config`, `crates/ulo-health`, `crates/ulo-health-http`, `crates/ulo-health-grpc`, `crates/ulo-db`, `crates/ulo-db-sqlx`, `crates/ulo-db-seaorm`, `crates/ulo-db-diesel`, `crates/ulo-db-mongodb`, `crates/ulo-db-redis`, `crates/ulo-db-conformance`, `crates/ulo-conformance-support`, `crates/ulo-otel`, `crates/ulo-otel-otlp`; `exclude` loses the seven brought back and keeps `crates/ulo-db-prisma`, `integration-tests` and `examples`; the new workspace dependencies, at the versions the probes pinned: `toml` 1, `jsonc-parser` 0.34, `saphyr` 0.2 and `saphyr-parser` 0.2 (R2's pre-scan reads its events; `saphyr` re-exports none of them), one `sqlx` 0.9 entry with no default features, `sea-orm` 2.0 and `sea-orm-migration` 2.0, `diesel` 2.3, `diesel-async` 0.9, `diesel_migrations` 2.3, `mongodb` 3.9, `opentelemetry`, `opentelemetry_sdk`, `opentelemetry-otlp` and `opentelemetry-http` 0.33.1 with no default features, `tracing-opentelemetry` 0.34.0, `tracing-subscriber` 0.3, `ureq` `=3.4.2`, pinned exactly (N2); `redis`, `tonic-health`, `futures-channel` and `serde` are entries already |
| `runtime-levels.txt` | the entries listed below |
| each crate's `Cargo.toml` and `src/**` in the tables of C, H1, H2, D1, D2, D3, S2a–S2d, O2 and O3 | the module tree and every public signature with `todo!()` bodies |

A crate on `master` that an area rewrites in place is emptied to its skeleton by S-skel: its old
`src/**`, `tests/**`, `examples/**` and `README.md` are removed and the new module tree written, as
the 2b spine did, so an area never reads old code as if it were the spec. The seven are
`ulo-config`, `ulo-health`, `ulo-db-sqlx`, `ulo-db-seaorm`, `ulo-db-diesel`, `ulo-db-mongodb` and
`ulo-db-redis`. Nothing of `master`'s implementation is carried over. `crates/ulo-db-prisma` is not
ported and not touched by the spine; the coordinator deletes it and its `exclude` line after the
race (decision 42).

S-skel writes 90 files, the 88 skeletons listed in the area tables and the 2 workspace files,
besides the removed `master` files of the seven rewritten crates. The spine writes 138 in all.

Interface. S-skel codes against these, never against S-core's files, which are being written beside
it:

- `integrations/DESIGN.md` §9, for which core item each extension adds and where;
- the cross-area contracts below, for every item one area calls in another, and the area tables, for
  each file's items;
- S-core's fixed surface above, for every core and `ulo-grpc` item a skeleton names in a signature
  or a manifest: `ulo`'s `serde` feature in `ulo-config`'s manifest, `TraceSource` and
  `TraceContext` in `ulo-otel`'s `source.rs`, `GrpcHealth` in `ulo-health-grpc`, `Permanent` where
  `ulo-db` names it;
- the existing core API for everything else (`Module`, `ModuleDef`, `try_value`, `try_singleton`,
  `Dep`, `Many`, `Bound`, `Timer`, `Runtime`, `spawn_with`, `Keyed`, `BoxError`, `BoxFuture`), as
  on the race-2b tree;
- `ulo-conformance-support`'s items at the names they have in `crates/ulo-rpc-conformance` today,
  which S-move keeps.

A signature S-skel needs and none of these fixes is a divergence S-skel logs and chooses; S-core's
surface is never one.

Design sections: integrations DESIGN §1, §4.1, §8, §9 (I10); fourth response P8; fifth response N2.

### Floors

S-skel writes each floor into its crate's manifest. The CI `msrv` job reads each declared `rust-version` and checks the crates declaring it with
`--all-features`, so a crate's floor covers every feature it has.

| Crate | `rust-version` | From |
|---|---|---|
| `ulo-config` | 1.88 (workspace) | `toml` 1.1.8, `saphyr` 0.2.2 and `saphyr-parser` 0.2.2 declare 1.85, `jsonc-parser` 0.34.0 none (read in the registry); P4's `saphyr` replaces `serde-saphyr`'s 1.89, so the `yaml` feature needs no floor of its own |
| `ulo-health`, `ulo-health-http`, `ulo-health-grpc` | 1.88 | |
| `ulo-db` | 1.88 | P1: no lock primitive |
| `ulo-db-sqlx` | 1.94 | SQLx 0.9.0 (PROBES_db.md, versions) |
| `ulo-db-seaorm` | 1.94 | SeaORM 2.0.4 |
| `ulo-db-diesel` | 1.89 | `std::fs::File::lock` for its SQLite sidecar lock (P1, integrations DESIGN §1; PROBES_db.md §7.2); diesel 2.3.14 declares 1.86 and diesel-async 0.9.2 1.84 |
| `ulo-db-mongodb` | 1.88 | mongodb 3.9.1 declares 1.88 |
| `ulo-db-redis` | 1.88 | redis 1.7.1 declares 1.88 |
| `ulo-db-conformance`, `ulo-conformance-support` | 1.88 | |
| `ulo-otel` | 1.88 | `opentelemetry`, `opentelemetry_sdk` 0.33.1 and `tracing-opentelemetry` 0.34.0 declare 1.75 (read in the registry) |
| `ulo-otel-otlp` | 1.88 | `opentelemetry-otlp` 0.33.1 declares 1.75, `ureq` 3.4.2 1.85 (read in the registry) |

`ulo-db-sqlx` and `ulo-db-seaorm` state their floor as `ulo-http-salvo` states 1.92. The `msrv`
matrix gains a 1.94 leg from the manifests, with no change to `ci.yml`.

### Level entries

S-skel writes sixteen lines, each under a comment saying why, as the file's other entries are:

```text
ulo-config                    runtime-free           --all-features
ulo-health                    runtime-free           --all-features
ulo-health-http               runtime-free           --all-features
ulo-health-grpc               tokio-bound
ulo-db                        runtime-free           --all-features
ulo-db-sqlx                   tokio-bound
ulo-db-seaorm                 tokio-bound
ulo-db-diesel                 tokio-bound
ulo-db-mongodb                tokio-bound
ulo-db-redis                  tokio-bound
ulo-db-conformance            runtime-free
ulo-conformance-support       runtime-free
ulo-conformance-support       runtime-free           --features smol
ulo-otel                      runtime-free           --all-features
ulo-otel-otlp                 runtime-free
ulo-otel-otlp                 tokio-bound            --features reqwest-blocking
```

`ulo-health-http` depends on `ulo-http` at default features, without its `tokio-io` feature, which
is what keeps `--all-features` runtime-free. `ulo-db-conformance` and `ulo-conformance-support` are
read at default features, as `ulo-rpc-conformance` is, their `tokio` feature bringing the harness;
`ulo-otel-otlp` at default features, its `reqwest-blocking` feature bringing tokio through reqwest
(PROBES_obs.md §4a). `ulo-otel` is read with `--all-features` provided no feature of its own enables
`opentelemetry_sdk`'s `rt-*`, `experimental_*` or `testing` features (PROBES_obs.md §3a).

## Areas and files

Every source file of the race's crates belongs to exactly one area. A file the spine finished and
no area table lists stays as the spine wrote it. Each area lists what it replaces on `master`, the
excluded workspace members it brings back, and the crates it creates.

### C. Configuration (stage 1)

Model: Sonnet. A new crate, but the design specifies each source, format and refusal, and nothing in
it runs concurrently or on a clock.

| Files | Holds |
|---|---|
| `crates/ulo-config/Cargo.toml`, `src/lib.rs` | the crate's manifest and re-exports; `ulo` with its `serde` feature, `serde`, `toml`, `jsonc-parser`, `saphyr` and `saphyr-parser` behind `yaml`; `tracing` for the unread-key warning |
| `crates/ulo-config/src/config.rs` | `Config`, `Config::builder()`, `.source(..)`, `.load()`, which never fails and records each source's failure inside the value |
| `crates/ulo-config/src/source.rs` | the source trait and the five sources: `Defaults`, `File::toml(..)` and its JSON and YAML siblings with `.optional()`, `DotEnv::file(..)` with `.optional()`, `Env::process()` and `Env::from_map(..)` with `.prefix(..)` and `.separator(..)`, `Values::new().set(..)`; layering in the order added |
| `crates/ulo-config/src/tree.rs` | the value tree whose every leaf carries its origin: a file with line and column, a variable's name, an explicit value, a default |
| `crates/ulo-config/src/format/mod.rs`, `toml.rs`, `json.rs`, `yaml.rs`, `dotenv.rs` | the four readers into the tree: `toml`'s spanned parsing; `jsonc-parser` in strict mode; `saphyr`'s `MarkedYaml`, read only after R2's pre-scan of `saphyr-parser`'s events through a `SpannedEventReceiver` has passed: the nodes inside each anchored node counted as its events arrive, an alias inside one counting at its anchor's size, each alias adding its anchor's count, a total past 100,000 nodes a problem naming the line of the alias that passed it, a fixed limit with no setting; `<<` refused naming the line, the YAML 1.2 core schema (P4); the `.env` grammar of P5, a key twice in one file a problem naming both lines, no expansion, nothing written to the process environment |
| `crates/ulo-config/src/de.rs` | the deserializer over the tree collecting every missing, ill-typed and out-of-range value; its error type, whose `invalid_type`, `invalid_value` and `unknown_variant` render the kind of the value found and never the value, which serde's buffered content under `flatten` and `untagged` raises through too (R3); `Secret` recognised by its newtype name and recorded with path and origin; P6's collector installed around each deserialisation through `ulo::__private::collect_secrets`; one leaf-type error per section (P7) |
| `crates/ulo-config/src/problems.rs` | `Problems`, `Problem`, `at(path, message)`; a problem under a path recorded by name as a secret's rendered from path and kind alone, a leaf type's own message included; elsewhere a leaf type's own `Error::custom` message printed as written, the limit the crate's documentation states (R3); the `Display` with a header line and one problem per line with path and origin (decision 8) |
| `crates/ulo-config/src/section.rs` | the section read every integration's `from_config` calls: deserialise the section at `path`, prefix its problems with the path, hand back the recorded secrets for `m.secret(..)`, record a failure through `try_value` |
| `crates/ulo-config/src/module.rs` | `ConfigModule::<T>::new(&config)`, `.validate(..)`, `.deny_unread()`; registration through `m.try_value(..)` and `m.secret(..)`; the init hook logging each unread leaf with its origin at `warn`, or refusing under `.deny_unread()` |
| `crates/ulo-config/src/error.rs` | the error `try_value` records, its structured list reachable through `Redacted::downcast_ref` |

Replaces on `master`: `ConfigModule<T: Config>` over `FromEnv` and `Validate`, `ConfigService<T>`,
`ConfigServiceFactory`, `Environment`, the `dotenv`-crate loading, and the `#[derive(Config)]` the
branch's `ulo-macros` no longer carries. Rewrites in place: `ulo-config`. Brings back: `ulo-config`.
Creates nothing.

Design sections: §2, §7 (the file-format rows), §10 (the configuration rows), decisions 1–8, §13.1;
fourth response P4, P5, P6, P7; fifth response R2, R3; PROBES_config.md §2–§7.

### H. Health

#### H1. `ulo-health` and `ulo-health-http` (stage 1)

Model: Opus. The before-shutdown hook and its bound, the readiness flip ahead of the first refusal,
the bounded concurrent run with probes sharing the run in flight, all on the app's `Timer`.

| Files | Holds |
|---|---|
| `crates/ulo-health/Cargo.toml`, `src/lib.rs` | the crate's manifest and re-exports |
| `crates/ulo-health/src/check.rs` | `Check` with `name`, `check`, `bound` (`Default` 2 s, `After(ZERO)` refused, `Unbounded` the opt-out) and `probe`; `Health::{Up, Down(reason)}`, the reason redacted; `Probe::{Readiness, Liveness}` |
| `crates/ulo-health/src/run.rs` | the bounded concurrent run on `Dep<dyn Timer>`: a check past its bound is down, a probe arriving mid-run shares that run, no result kept between runs |
| `crates/ulo-health/src/readiness.rs` | `Readiness` and its read methods: liveness, readiness and startup with Kubernetes' meanings; false from `Stopping`; the change notice the gRPC bridge subscribes to |
| `crates/ulo-health/src/module.rs` | the health module reading `Many<dyn Check>`; the zero-bound refusal at `connect`; `BeforeApplicationShutdown` turning readiness false, then waiting `.lag(d)`, bounded `After(d + 1 s)` |
| `crates/ulo-health-http/Cargo.toml`, `src/lib.rs` | the crate's manifest and re-exports; `ulo-http` at default features |
| `crates/ulo-health-http/src/routes.rs` | `HealthHttp::new()`: `/health/live`, `/health/ready`, `/health/started`, each configurable; 200 or 503 |
| `crates/ulo-health-http/src/body.rs` | `application/health+json`, `status` `pass` or `fail`; `.detail(true)` adding the draft's `checks` object with redacted reasons |

Replaces on `master`: `TerminusModule`, `HealthCheckService`, `HealthIndicator`, `HealthEntry`,
`HealthCheckResult` and the `disk`, `memory` and `http` indicators behind features. The design has
no indicators, and H1's log records their retirement. Rewrites in place: `ulo-health`. Brings back:
`ulo-health`. Creates: `ulo-health-http`.

Design sections: §3.1–§3.3, §3.5, §7 (the Kubernetes and IETF rows), §10 (the health rows),
decisions 9, 10, 14–17, 41; fourth response P10; PROBES_obs.md §6.

#### H2. `ulo-health-grpc` (stage 3)

Model: Opus. An init hook that must run before `listen()`, a re-evaluation task on the app's
runtime, and writes ordered against the server's own.

| Files | Holds |
|---|---|
| `crates/ulo-health-grpc/Cargo.toml`, `src/lib.rs` | the crate's manifest and re-exports; `ulo-grpc`, `ulo-health`, `tonic-health` |
| `crates/ulo-health-grpc/src/bridge.rs` | the bridge module: its init hook calls `GrpcHealth::delegate_overall()` before `listen()`, then writes `""` from `Readiness` through `Dep<GrpcHealth>`, `NOT_SERVING` at `Stopping` and whenever a check is down; every write goes through `GrpcHealth`'s stored copy, which reaches the reporter only on a change (S4), so the bridge keeps no copy of its own |
| `crates/ulo-health-grpc/src/interval.rs` | the re-evaluation task on `Dep<dyn Runtime>`: a `Bound`, `Default` 5 s, `Unbounded` meaning lifecycle transitions alone, `After(ZERO)` refused as `ping_interval`'s zero is |

Replaces nothing on `master`. Creates: `ulo-health-grpc`.

Design sections: §3.4, §3.5 item 5, decisions 11–13, 59; fourth response P9; fifth response S4;
PROBES_obs.md §1; transports DESIGN §6.2, decision 11.

### D. Databases: the shared policy, the reference store and the suite (stage 2)

#### D1. `ulo-db`

Model: Opus. The connect policy over `.ready`, the `CREATE_CONNECTS` loop and its stop on
`Permanent`, the migration lock's order and bound, the close after the drain.

| Files | Holds |
|---|---|
| `crates/ulo-db/Cargo.toml`, `src/lib.rs` | the crate's manifest and re-exports; `ulo`, `ulo-config`, `ulo-health`, `serde`; no driver |
| `crates/ulo-db/src/store.rs` | `Store`: `NAME`, `Options`, `Client`, `CREATE_CONNECTS`, `validate`, `create`, `ping`, `close` (§4.1, H1) |
| `crates/ulo-db/src/module.rs` | `StoreModule<S>`, its identity `ModuleIdentity::of_value` over `S::Options`; `from_config(&config, path)`; `.keyed::<Q>()` through `Keyed`; `.lazy()` where offered, absent at compile time elsewhere; `.migrate(..)`; `.without_health_check()`; `.connect_within(d)`, the total connect bound, 30 s by default, a zero refused at registration; `.retries(..)` and `.backoff(..)`; registration: `validate` through `try_value`, the secrets, `try_singleton`, for a `CREATE_CONNECTS` store its construction bound set to `connect_within` through `.timeout(..)` on the binding handle, `.ready(..)` unless lazy (S1), the contributed check, `.on_destroy(close)`, the bare `S::Client` export |
| `crates/ulo-db/src/connect.rs` | the factory: `Dep<dyn Runtime>`'s `name()` compared with `"tokio"` before anything is built, `create` through `spawn_with`; one retry loop per store (S1): for a store whose `create` connects nothing, `.ready(ping).retries(..).backoff(..).timeout(connect_within)`; for a `CREATE_CONNECTS` store, the loop around `create` with the module's retries and backoff, ending on `Permanent`, under the construction bound `connect_within`, then `.ready(ping).retries(0)` under the default attempt bound as one confirming ping; the ping and the close through `spawn_with` |
| `crates/ulo-db/src/migrate.rs` | `Migrations`; the store-side lock trait (the dedicated connection and the primitive); the protocol: the key `K`, lock before the migrator and release after, under the module's bound, failures as `MigrationFailed`; run in the module's `on_init` after connecting |
| `crates/ulo-db/src/health.rs` | the `Check` named from `S::NAME`, running `ping` |
| `crates/ulo-db/src/error.rs` | `UnsupportedRuntime { module, required, given }` and `MigrationFailed { module, source }`, each raised inside `Permanent` |

Replaces on `master`: the `CheckedModule` and `StartupCheck` of the old `ulo` core, on which every
`master` store crate built its connect, retry and health policy. Creates: `ulo-db`.

Design sections: §4.1–§4.5, §4.7, §4.8, §10 (the store rows), decisions 18–24, 27, 48; fourth
response P1, H1, H3; fifth response S1, N3; DESIGN §8.3, §8.6, §9.2, §9.3, §9.5.

#### D2. `ulo-db-sqlx`

Model: Sonnet. A port specified to the SQLSTATE: one `Store` per engine, its error table, its lock
primitive.

| Files | Holds |
|---|---|
| `crates/ulo-db-sqlx/Cargo.toml`, `src/lib.rs` | `rust-version = "1.94"`; the workspace `sqlx` with `runtime-tokio` alone, `migrate`, and the features `postgres`, `mysql` and `sqlite`; the TLS feature, on by default, enabling SQLx's `tls-rustls-ring-native-roots`: ring with the platform's roots (R1), and no `mysql-rsa`; `pub use sqlx`; the crate's module name for `StoreModule<S>` |
| `crates/ulo-db-sqlx/src/options.rs` | `PgOptions`, `MySqlOptions`, `SqliteOptions`: the URL a `Secret<String>`, pool sizes, `Eq + Hash`, a `Debug` writing the URL redacted; the scheme check `ulo-db-seaorm` shares (P3); validation through each engine's `*ConnectOptions::from_str`, a failure a problem at `url` written from its category with the userinfo replaced |
| `crates/ulo-db-sqlx/src/postgres.rs`, `src/mysql.rs`, `src/sqlite.rs` | `Postgres`, `MySql`, `Sqlite` as `Store`s: `connect_lazy_with` in `create`, `.lazy()` offered, `select 1` in `ping`, `close`; each engine's lock primitive on a connection held for the migration (H3): `pg_advisory_lock(K)`, `GET_LOCK` on a name built from `database()` and hashed past 64 characters, `File::lock` on `<file>.ulo-migrate.lock` taken off the worker, none for in-memory SQLite |
| `crates/ulo-db-sqlx/src/classify.rs` | `permanent_or_not`: `Configuration`, and SQLSTATE class `28` or `3D000` on Postgres, `28000` or `42000` on MySQL, `14` on SQLite, wrapped in `Permanent`; `Io`, `PoolTimedOut`, `WorkerCrashed`, `Tls` bare |
| `crates/ulo-db-sqlx/src/migrate.rs` | `Migrations` for `sqlx::migrate::Migrator`, run inside `ulo-db`'s lock; SQLx's own lock nests inside it under its own key |

R1's premise, read in the registry: SQLx 0.9.0's `tls-rustls-ring-native-roots` enables
`sqlx-core`'s `_tls-rustls-ring-native-roots`, which brings `rustls-native-certs`, and its rustls
configuration is built with the provider it names (`sqlx-core` 0.9.0, `src/net/tls/tls_rustls.rs:99-107`).
ring is the provider of the root `Cargo.toml`'s `rustls`, `tokio-rustls` and `futures-rustls`
entries. The crate's documentation states that rumqttc brings aws-lc-rs, so an application linking
`ulo-rpc-mqtt` links both providers whatever this crate enables.

Replaces on `master`: `SqlxModule::postgres`, `postgres_named` and the other constructors returning
`CheckedModule`, the pool wrapper, the `health` feature's indicator, the crate's own URL redaction,
SQLx 0.8. Rewrites in place: `ulo-db-sqlx`. Brings back: `ulo-db-sqlx`. Creates nothing.

Design sections: §4.1 (the worked store), §4.2, §4.4, §4.5, §4.6 (the SQLx rows), decisions 23, 26;
fourth response P1, P3, H3, H6; fifth response R1, R5; PROBES_db.md §1, §6, §7, §8.1.

#### D3. `ulo-db-conformance`

Model: Opus. Scenarios of timing and order: the stall on one thread, the pool closing after the
drain with a query in flight, two replicas racing to migrate.

| Files | Holds |
|---|---|
| `crates/ulo-db-conformance/Cargo.toml`, `src/lib.rs` | `publish = false`; runtime-free at default features, a `tokio` feature bringing `ulo-conformance-support`'s `OnTokio`; the host trait a store's tests implement (a server or a file, the options reaching it, options that cannot reach it, options with bad credentials, an outage and its recovery through the support crate's `Relay`, the migrations the lock scenario runs, a `Budget`); the capabilities `lazy` and `migrations`, named positively and true by default; the stamping macro with `not_applicable` under the support crate's rules |
| `crates/ulo-db-conformance/src/cases/mod.rs`, `wire.rs`, `connect.rs`, `lazy.rs`, `health.rs`, `close.rs`, `stall.rs`, `runtime.rs`, `keyed.rs`, `migrate.rs` | the ten scenarios of §4.9, `connect.rs` holding both the unreachable and the bad-credentials scenario; each names the planted break it is shown failing against |

Depends on D1's `Store` and `StoreModule` and on H1's `Readiness`. Creates: `ulo-db-conformance`.

Design sections: §4.9, decision 29; I11; transports DESIGN §3.8 (the `not_applicable` rules),
§5.2 (the RPC suite this one is shaped like).

### S2. The other stores (stage 3)

Each sub-area depends on D1's contracts. S2a also depends on D2's.

#### S2a. `ulo-db-seaorm`

Model: Sonnet. A port beside D2, on the same SQLx.

| Files | Holds |
|---|---|
| `crates/ulo-db-seaorm/Cargo.toml`, `src/lib.rs` | `rust-version = "1.94"`; `sea-orm` with its tokio runtime and SQLx drivers, `sea-orm-migration`; `ulo-db-sqlx` for the shared scheme check and the one SQLx entry; the crate's module name |
| `crates/ulo-db-seaorm/src/options.rs` | the options: the URL a `Secret<String>`, a redacted `Debug`; validation with SQLx's option parser after the shared scheme check, so no malformed URL reaches `Database::connect` (P3) |
| `crates/ulo-db-seaorm/src/store.rs` | `Store` for `DatabaseConnection`: `ConnectOptions::connect_lazy(true)`, `.lazy()` offered; the whole URL and its password component both registered with `m.secret(..)` (P3, measure 1); errors classified through the SQLx error SeaORM carries, as D2's table |
| `crates/ulo-db-seaorm/src/migrate.rs` | `Migrations` for `sea-orm-migration`, which takes no lock, under `ulo-db`'s lock; the per-engine primitive on a connection from the pool SeaORM holds |

Replaces on `master`: the SeaORM 1 module over `CheckedModule`, its health indicator, its own
redaction. Rewrites in place and brings back: `ulo-db-seaorm`.

Design sections: §4.2 (SeaORM's echoing errors), §4.4, §4.5, §4.6, decision 26; fourth response P3,
H3; PROBES_db.md §2.

#### S2b. `ulo-db-diesel`

Model: Opus. Where blocking work runs: `Given` over the given runtime's handle for SQLite and the
migrations, the handle captured inside `create`'s task, the sidecar lock.

| Files | Holds |
|---|---|
| `crates/ulo-db-diesel/Cargo.toml`, `src/lib.rs` | `rust-version = "1.89"`; `diesel`, `diesel-async` with `deadpool`, `diesel_migrations`; features `postgres`, `mysql`, `sqlite`; the crate's module name |
| `crates/ulo-db-diesel/src/options.rs` | the options: the URL a `Secret<String>`, a redacted `Debug`, the scheme check |
| `crates/ulo-db-diesel/src/store.rs` | `Store` for a deadpool `Pool` of diesel-async connections; `build` connects nothing, so `.lazy()` is offered (H1); `Given`, `pub(crate)`, a `SpawnBlocking` over a `tokio::runtime::Handle`, `spawn_blocking` calling `handle.spawn_blocking(..)` (R4); the handle read once with `Handle::current()` inside `create`'s future, which the factory runs on the given runtime through `spawn_with` after the name check; SQLite through `SyncConnectionWrapper`, each connection established in `custom_setup` through `Given::spawn_blocking` and wrapped with `with_runtime(conn, Given(handle.clone()))`, so neither `SyncConnectionWrapper::new` nor `establish`, the two callers of `get_runtime()`, is reached |
| `crates/ulo-db-diesel/src/classify.rs` | diesel-async's errors into permanent and retryable: wrong credentials and an unknown database both arrive as `DatabaseError(Unknown, message)` with the SQLSTATE lost; `UnableToSendCommand` and `BadConnection` retryable |
| `crates/ulo-db-diesel/src/migrate.rs` | `diesel_migrations` through `AsyncConnectionWrapper`, its closure run through the same `Given` on the given runtime's blocking pool (R4); `AsyncConnectionWrapper` takes no `BlockOn` value, and its tokio `BlockOn` reads `Handle::try_current()` on that pool's thread, the given runtime (DESIGN §13.2) |
| `crates/ulo-db-diesel/src/lock.rs` | the three primitives over diesel-async's connections, the SQLite sidecar lock off the worker |

The workspace resolves one `libsqlite3-sys` across SQLx 0.9.0 (`>=0.30.1, <0.38.0`) and diesel
2.3.14 (`>=0.17.2, <0.39.0`), read in the registry.

Replaces on `master`: the diesel-async 0.8 module over `CheckedModule`, its pool wrapper, health
indicator and redaction. Rewrites in place and brings back: `ulo-db-diesel`.

Design sections: §4.6 (the Diesel paragraph), decision 25; fourth response P1, H1, H2, H3; fifth
response R4, N1; PROBES_db.md §3, §8.2.

#### S2c. `ulo-db-mongodb`

Model: Sonnet.

| Files | Holds |
|---|---|
| `crates/ulo-db-mongodb/Cargo.toml`, `src/lib.rs` | `mongodb` 3.9; the crate's module name; no `.lazy()`, no `.migrate` |
| `crates/ulo-db-mongodb/src/options.rs` | the options: the URI a `Secret<String>`, a redacted `Debug`; `validate` checking the scheme, `mongodb://` or `mongodb+srv://`, and a host, with no lookup (H6) |
| `crates/ulo-db-mongodb/src/store.rs` | `Store` for `mongodb::Client`: `CREATE_CONNECTS = true`, a `const` for the whole store (S2), so a `+srv` DNS failure is retried by D1's loop under `connect_within` and a plain `mongodb://` URL succeeds on the loop's first attempt; `ClientOptions::parse` and the SRV lookup in `create`, on the app's runtime through `spawn_with`; no `.lazy()`, `CREATE_CONNECTS` being true, though `Client::with_options` connects nothing; `ErrorKind::Authentication` and `InvalidArgument` permanent, `ServerSelection` retryable |

Replaces on `master`: the mongodb module over `CheckedModule`, its health indicator and redaction.
Rewrites in place and brings back: `ulo-db-mongodb`.

Design sections: §4.6, decisions 48, 53; fourth response H6; fifth response S1, S2; PROBES_db.md §4,
§8.3.

#### S2d. `ulo-db-redis`

Model: Sonnet. The retry loop around a connecting `create` is D1's.

| Files | Holds |
|---|---|
| `crates/ulo-db-redis/Cargo.toml`, `src/lib.rs` | the workspace `redis` with `connection-manager`, `tokio-comp` alone; the crate's module name; no `.lazy()`, no `.migrate` |
| `crates/ulo-db-redis/src/options.rs` | the options: the URL a `Secret<String>`, a redacted `Debug`, `redis://` and `rediss://`, validated through `Client::open` |
| `crates/ulo-db-redis/src/store.rs` | `Store` for `ConnectionManager`, `CREATE_CONNECTS = true`, `ConnectionManagerConfig::set_number_of_retries(0)` (H1); `prefer_tokio()` once per process under a process-wide `OnceLock` of the crate's, called by the first pool factory at `connect` after the name check, its outcome recorded for every later Redis module (S3): `Ok` continues; a refusal, another crate's preference set first, is read where redis-rs allows, tokio continuing and anything else a `Permanent` refusal at `connect` naming the conflict, and where it cannot be read, a `warn` naming the conflict and the start continuing; `AuthenticationFailed` and `InvalidClientConfig` permanent, `Io` and a timeout retryable |

Replaces on `master`: the redis module over `CheckedModule`, its health indicator and redaction.
Rewrites in place and brings back: `ulo-db-redis`.

redis 1.7.1 offers no read of the preference, and offers `prefer_tokio()` only under
`cfg(all(feature = "smol-comp", feature = "tokio-comp"))` (`src/aio/runtime.rs:93-105`), which a
crate enabling `tokio-comp` alone cannot call (DESIGN §13.2). That premise is a design question
returned before stage 3; S2d builds what the designer answers.

Design sections: §4.5, §4.6, decision 60; fourth response H1, H6; fifth response S1, S3;
PROBES_db.md §5, §8.4.

### L. CLI templates (stage 4)

Model: Sonnet.

| Files | Holds |
|---|---|
| `crates/ulo-cli/Cargo.toml`, `src/main.rs`, `src/commands/mod.rs` | `new`'s `--transport` and `--db` flags; `generate`'s five kinds |
| `crates/ulo-cli/src/commands/new.rs` | the project from templates compiled in with `include_str!`, no network fetch; `ulo = "=<the CLI's version>"`, an exact pin where `src/templates/new/Cargo.txt` writes a caret requirement on `0.1.0-alpha.1` today |
| `crates/ulo-cli/src/commands/generate.rs` | `module`, `provider`, `controller`, `gateway`, `resolver`: new files only, the registration line printed, an existing file or a name that is no identifier refused; `update_app_module` (`:136`) and `update_mod_file` (`:166`) removed |
| `crates/ulo-cli/src/templates/**` | one `new` template per transport, an optional database layer per store, and the five `generate` templates, all written against race 3's APIs; the current `new` template targets the `master`-era `ulo-http-axum` and is replaced |
| `crates/ulo-cli/README.md` | the `new` and `generate` sections, which the change falsifies |

`commands/dev.rs` and `commands/exec.rs` are untouched; `ulo dev` is unchanged. The CI job that
builds every template and combination and fails one whose tests run nothing is the coordinator's,
and L's log describes it as owed. Replaces on `master`: the `resource` generator and its edits of
existing source. Rewrites in place: `ulo-cli`'s `new` and `generate`. Creates nothing.

Design sections: §5, decisions 30, 31; transports DESIGN §1 (the `generated code lints` job).

### O. Observability (stage 5)

#### O1. The carriers in the four hubs

Model: Opus. Order: extract before `Execution::open`, the WebSocket connection's context as each
message's parent, the merge-or-forward rule of H4 and S5 against the reservation S-core wrote.

| Files | Holds |
|---|---|
| `crates/ulo-http/src/service.rs`, `src/trace.rs` | extraction from the request's headers into `ExecOptions::trace`; `otel.kind` SERVER; `otel.status_code` on a 5xx and not on a 4xx |
| `crates/ulo-grpc/src/dispatch.rs`, `src/client.rs`, `src/trace.rs` | extraction from the request's metadata; injection into an outgoing request's metadata in `outgoing`; the status rule for gRPC codes |
| `crates/ulo-rpc/src/dispatch.rs`, `src/client.rs`, `src/trace.rs` | extraction from `CallHeaders`, events through the same path, so an event's handler span has the emitter's as its parent; injection into the frame's headers in `written` on every shape, events included, `tracestate` merged by W3C §3.5 when the context's `TraceOrigin` is `Originated`, and the text as received forwarded when it is `Received` (H4, S5); the status rule for RPC |
| `crates/ulo-ws/src/connection.rs`, `src/gateway.rs`, `src/trace.rs` | extraction from the handshake's headers; the connection's context as the parent of every message's execution and of the terminal one; the status rule for WebSocket |

An invalid incoming context gives no context in pass-through and never fails the request (H4).
Changes no crate's membership. The `Spec::reserved` rule in `ulo-rpc/src/client.rs` is S-core's
and stays as written. Each hub reads the source through `AppHandle::trace_source()` as `span::call`
takes it (S5).

Design sections: §6.1–§6.3, decisions 32–36, 56, 57; fourth response H4, H5; fifth response S5;
PROBES_obs.md §5, §7.

#### O2. `ulo-otel`

Model: Opus. The flush on a thread of its own under the hook's bound at `OnApplicationShutdown`,
and `set_parent` before the span is first entered.

| Files | Holds |
|---|---|
| `crates/ulo-otel/Cargo.toml`, `src/lib.rs` | `opentelemetry` with no default features, `opentelemetry_sdk` with `trace` alone, `tracing-opentelemetry` 0.34.0, `tracing-subscriber`, `futures-channel`, `ulo-config` |
| `crates/ulo-otel/src/otel.rs` | `Otel::new(options)`, built in `main`; `.layer()` for the application's subscriber; `.module()` |
| `crates/ulo-otel/src/options.rs` | `OtelOptions` from a configuration section, or from the `OTEL_*` names read from a given `Env` |
| `crates/ulo-otel/src/source.rs` | the `TraceSource` implementation, `start(&self, span: &Span, inbound: Option<&TraceContext>) -> TraceContext`: the inbound context into an `opentelemetry::Context`, `OpenTelemetrySpanExt::set_parent`, then `span.context()` read back as the execution's own context, which `set_trace` marks `Originated`; the source the app resolves once at `connect`, called by `span::call` and by `execute` on an `ExecOptions::span` (S5) |
| `crates/ulo-otel/src/sampler.rs` | parent-based sampling by default; a remote parent with the sampled flag clear exports nothing |
| `crates/ulo-otel/src/flush.rs` | the batch processor's `shutdown` on a thread of its own, its end signalled through a `futures-channel` oneshot the `OnApplicationShutdown` hook awaits under its bound; on expiry the thread left running, detached, the hook recorded `TimedOut` |
| `crates/ulo-otel/src/exporter.rs` | the exporter slot `ulo-otel-otlp` fills |

Creates: `ulo-otel`.

Design sections: §6.4, decisions 37, 39, 57, 58; fourth response H5; fifth response S5;
PROBES_obs.md §2, §3.

#### O3. `ulo-otel-otlp`

Model: Sonnet.

| Files | Holds |
|---|---|
| `crates/ulo-otel-otlp/Cargo.toml`, `src/lib.rs` | `opentelemetry-otlp` with `http-proto` and `trace` alone, `opentelemetry-http`, the workspace `ureq`, pinned `=3.4.2` (N2); the `reqwest-blocking` feature enabling `reqwest-blocking-client`; `OtlpExporter::http()` |
| `crates/ulo-otel-otlp/src/client.rs` | the `HttpClient` over `ureq`, blocking inside its async `send_bytes` on the batch thread; its rustls client built with an explicit provider through `TlsConfig::unversioned_rustls_crypto_provider`, outside ureq's semver guarantee, or construction refused with transports S7's message (P8) |

Creates: `ulo-otel-otlp`.

Design sections: §6.4 (export), decisions 38, 61; fourth response P8; fifth response N2;
PROBES_obs.md §4.

### Not in race 3

- `crates/ulo-db-prisma`: not ported (decision 42); the coordinator deletes it after the race.
- `integration-tests` and `examples` stay excluded.
- Every test, CI job and script change, which the coordinator writes after the race.
- The `master` health indicators (`disk`, `memory`, `http`), which the design does not have.
- `ulo dev`, unchanged; its Windows gap stays F305.
- DESIGN §12's deferrals: reloading configuration while running; remote configuration sources;
  transactions scoped to an execution; per-test databases; smol variants of the stores;
  exponential backoff on connect; OTLP export over gRPC; the exit codes 78 and 75; a child span
  opened in a handler and passed on explicitly; a second writer for named gRPC services; publishing
  `ulo-db-conformance` (F403); a zero bound on the core's own hooks, constructions and readiness
  checks (F313).

## Cross-area contracts

Each row is an item one area calls in another; the owner keeps it as the spine writes it.

### Owned by S-core and S-move

The items S-core owns keep the surface S fixes for them.

| Item | Owner | Called by | For |
|---|---|---|---|
| `Spawn::name` | S-core | D1 | the runtime check in the pool's factory |
| `Permanent`, `is_permanent` | S-core | D1, D2, S2a–S2d, D3 | permanent `create` and `ping` errors, `UnsupportedRuntime`, `MigrationFailed`, the suite's assertions |
| the readiness loop's end on `Permanent` | S-core | D1 | bad credentials stop retrying |
| `Secret<T>: Deserialize`, `ulo::__private::collect_secrets` and `CollectedSecrets` | S-core | C | recording every secret leaf |
| `TraceContext`, `TraceState`, `TraceOrigin`, `TraceMode`, `ExecOptions::{trace, span}`, `Execution::{trace, set_trace}`, `ExecutionRef::trace`, `AppBuilder::trace`, `AppHandle::trace_source` | S-core | O1, O2 | extraction, injection, the `tracestate` rule, the source's answer |
| `TraceSource`, `span::call`'s signature, `otel.kind`, `otel.status_code` | S-core | O1 (call sites, status), O2 (implements `TraceSource`) | parentage and export |
| the helpers each hub's `trace.rs` declares | S-core | O1 | the carrier bodies |
| `GrpcHealth::{module, delegate_overall, set, set_named}` and the stored copy behind them | S-core | H2 | one writer for `""`; a write reaching the reporter only on a change |
| `ulo-conformance-support`'s moved items | S-move | D3, and the RPC suite | harness, budget, startup failures, relay, stamping rules |

### Owned by C

| Item | Called by | For |
|---|---|---|
| `Config`, its builder, the five sources, `Env` | D1, O2, L | `from_config`, `OTEL_*` from a given `Env`, the templates' `main` |
| the section read | D1, O2 | a section by path at registration |
| `Problems` and its constructors | D1, D2, S2a–S2d | `Store::validate` |
| `ConfigModule<T>` | L | the templates |

### Owned by H1

| Item | Called by | For |
|---|---|---|
| `Check`, `Health`, `Probe` | D1 | the store's contributed check |
| `Readiness` and its read methods | H2, D3 | the bridge; the lazy and outage scenarios |
| the health module, `.lag` | L, D3 | the templates; scenarios that serve health |
| `HealthHttp` | L | the templates |

### Owned by D1

| Item | Called by | For |
|---|---|---|
| `Store` with `NAME`, `Options`, `Client`, `CREATE_CONNECTS`, `validate`, `create`, `ping`, `close` | D2, S2a–S2d, D3 | every store |
| `StoreModule<S>`: `from_config`, `.keyed::<Q>()`, `.lazy()`, `.migrate(..)`, `.without_health_check()`, `.connect_within(..)`, `.retries(..)`, `.backoff(..)` | D2, S2a–S2d, D3, L | each store crate's module name; the suite; the templates |
| the migration lock protocol: `Migrations`, the store-side lock trait, the key, the order, the bound, the error | D2, S2a, S2b | the per-engine primitives |
| `UnsupportedRuntime`, `MigrationFailed` | D3 | the runtime and migration scenarios |

### Owned by D2

| Item | Called by | For |
|---|---|---|
| `ulo_db_sqlx::sqlx`, the module name, `Postgres`, `MySql`, `Sqlite`, the options types | S2a, L | one SQLx; the templates |
| the scheme check | S2a | P3 |

### Owned by D3

| Item | Called by | For |
|---|---|---|
| the host trait, the capabilities, the stamping macro | the coordinator's `tests/conformance.rs` in each store crate | the suite per store |

### Owned by O2

| Item | Called by | For |
|---|---|---|
| the exporter slot | O3 | `OtlpExporter` |

H2, S2a–S2d, L, O1 and O3 own nothing outward.

## Points each area resolves in its own log

Not settled by the designs or by the spine. The owning area decides and logs the decision. A point
that fixes a public signature is the spine's, since the spine freezes every signature; the area
that fills the item builds against what the spine chose. Points the fourth and fifth responses
settled, and the surface S fixes for S-core, are not listed.

- **S-core.** The parameters `span::call` gains to reach the execution and the source
  `AppHandle::trace_source()` answers; `TraceContext`'s methods beyond `origin`; how a hub reads
  `TraceMode`, or whether `Execution::open` drops an inbound context under `Off`; whether `execute`
  runs the closure inside the span `ExecOptions::span` hands it; whether `TestApp` carries
  `trace(..)`; what the collector records for a `T` that is not `Display`, behind
  `CollectedSecrets::register`; how `near_spelling` reads a key's full type name beside its
  `TypeId`; the lock `GrpcHealth`'s stored copy and its writes share.
- **S-skel.** The signatures §14 leaves to areas: the spelling of the JSON and YAML file
  constructors and whether JSON sits behind a feature; `Problems`' constructors; the health
  module's name and how `.lag` is written on it; `Readiness`'s read methods; whether `HealthHttp` is
  imported as a module or mounted as a controller; the error type of a zero `Check` bound; where
  the bridge's interval is set; whether each store crate's module is a type alias of
  `StoreModule<S>` or a newtype; how `.lazy()` is absent at compile time; the shape of `Migrations`
  and the store-side lock trait; how the exporter is handed to `Otel` and how `OtelOptions` is read
  from a section.
- **C.** How a section's problems are prefixed with its path; how `ConfigModule` learns which leaves
  each section read consumed; how `Defaults`, built before the application's type is known, reaches
  the tree; how a merge key is told apart in `MarkedYaml`.
- **H1.** What `Readiness` reads to know the app is `Running`; how the gRPC bridge is told of a
  change.
- **H2.** Whether `ulo-health-grpc` imports `GrpcHealth::module()` itself or requires the
  application to; where the interval's zero is refused.
- **D1.** The default `.retries(..)` and `.backoff(..)`; the literal `K`, computed as H3 states and
  written into the source; how two keyed instances' checks are named apart, `S::NAME` being one
  string.
- **D2.** How an attempt's bound is kept above an SQLx pool's `acquire_timeout`; the hash for a
  MySQL lock name past 64 characters; how the SQLite sidecar lock is taken off the worker.
- **D3.** How the drain scenario holds a query in flight, and which transport or standalone
  execution it uses; how the replica scenario starts two apps against one server.
- **S2a.** Whether the lock primitives reuse D2's over the SQLx pool SeaORM holds, which would need
  them public (a request to D2), or are written again.
- **S2b.** How diesel-async's errors are classified, by the driver's own connect in `ping` or by
  matched messages; how the SQLite sidecar lock is taken off the worker; what `Given::get_runtime`,
  which `SpawnBlocking` requires and the crate never reaches, does.
- **S2c.** Which failures of the SRV lookup in `create` are permanent.
- **S2d.** The bound of one `create` attempt inside D1's loop.
- **L.** The transport and database names `--transport` and `--db` accept; whether the no-tests
  count is factored out of `scripts/unsafe_allowed.py` (a request to the coordinator) or written
  again; what the `resolver` template depends on; whether a template pins its other `ulo-*`
  dependencies exactly, as it pins `ulo`.
- **O1.** Which gRPC codes, RPC errors and WebSocket outcomes count as a server span's error, the
  design stating the rule for HTTP alone; the `otel.kind` each non-HTTP span records, an RPC
  event's handler being a consumer of the producer's span (§6.2); where the WebSocket connection
  keeps its context.
- **O2.** The flush hook's bound; how `OTEL_*` names map onto `OtelOptions`.
- **O3.** Which provider `ureq`'s rustls client is given, the workspace's `rustls` entry naming
  `ring`.

## After the race

The coordinator:

1. Compiles the workspace stage by stage: the spine's tree first (the core, the runtimes,
   `ulo-transport`, the four hubs, both suite crates), then each stage's crates in order, with
   `cargo check --all-features --all-targets`, the `runtime levels` check against the spine's
   entries, and the `msrv` legs at 1.88, 1.89 and 1.94.
2. Writes the tests: each store crate's `tests/conformance.rs` invoking `ulo-db-conformance`
   (`ulo-db-sqlx` for Postgres, MySQL and SQLite on a file; `ulo-db-seaorm`; `ulo-db-diesel`;
   `ulo-db-mongodb`; `ulo-db-redis`), against containers behind an `integration` feature as the
   broker links' are; the health scenarios of §3.5 on every host that serves health; the trace
   scenarios of §6.5 on every transport suite with a recording tracer; unit tests for health's
   state before `listen` (§3.2), `TraceContext` against the W3C vectors (PROBES_obs.md §5), the
   raw text a `Received` context forwards, and the P2 help. Each scenario is shown failing against its planted break before it is believed.
3. Runs the premise checks: DESIGN §13's rows marked not probed (`CREATE INDEX CONCURRENTLY` under
   an advisory lock); R2's event pre-scan against a nested alias bomb, and `saphyr-parser`'s events
   carrying anchor IDs and positions; R3's error type against a failing `Secret` under `flatten` and
   `untagged`; `ureq`'s explicit provider (P8); R5's test, built with SQLx's `runtime-smol` beside
   `runtime-tokio`, dropping a `PoolConnection` on a thread with no tokio context and asserting no
   task ran on smol's executor, which DESIGN §13.2's reading of `sqlx-core` expects to fail; a build
   with redis-rs's `smol-comp` beside `tokio-comp`, and whether redis-rs lets the preference be
   read (S3); one SQLx in
   `cargo tree -d` with both SQL modules (decision 26); `cargo tree -i tokio` exiting non-zero for
   `ulo-otel` and for `ulo-otel-otlp` at default features (PROBES_obs.md §3a, §4c). A failed
   premise returns as a design question before the stage that rests on it is accepted.
4. Adds the CI jobs the design names: the two-SQLx check, the template matrix with its no-tests
   count, the store suites against containers.
5. Deletes `crates/ulo-db-prisma` and its `exclude` line.
6. Folds the fifteen logs (`divergences/race3-S.md` and one per area: C, H1, H2, D1, D2, D3, S2a,
   S2b, S2c, S2d, L, O1, O2, O3) into `integrations/DIVERGENCES.md`, and the designer answers them
   in `integrations/RESPONSE.md`.
