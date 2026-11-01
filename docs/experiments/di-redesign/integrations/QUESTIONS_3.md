# Questions on applying the fourth response (for the designer)

Written for: the designer. Folding the fourth response into `DESIGN.md` and writing `BUILD_PLAN_3.md` against the repository turned up rulings that conflict with a probe, the repo or the core. Each is applied as ruled and recorded in `DESIGN.md` §13 or §14. The ones below fix a public signature or change what a ruling claims, so they come before the spine.

Paste alongside: `DESIGN.md` §2.3, §3.4, §4.2, §4.6, §6, §13, §14; `BUILD_PLAN_3.md` stage 0 table.

## Rulings that do not hold as written

**R1. The provider the workspace uses is ring, not aws-lc-rs.** H6 says aws-lc-rs is "the provider the rest of the workspace uses". The root `Cargo.toml` pins `rustls`, `tokio-rustls`, `futures-rustls` and `rcgen` on ring, `ulo-grpc` uses tonic's `tls-ring`, and async-nats uses ring. aws-lc-rs comes in through rumqttc only. SQLx 0.9.0 names its provider itself (`tls_rustls.rs:99-107`), so its own connections are safe, but an application linking `ulo-db-sqlx` with TLS beside ring has both providers linked, which is F422's condition for async-nats. No SQLx 0.9.0 feature gives aws-lc-rs with the platform's roots: `_tls-rustls-aws-lc-rs` brings `webpki-roots`, and native roots come only with the ring feature.
- Which does `ulo-db-sqlx` enable: the ring feature with platform roots (matches the workspace; no second provider), or aws-lc-rs with webpki roots as H6's fallback has it?

**R2. The alias bound cannot be enforced by the conversion.** P4 says the conversion into the tree enforces a bound on alias expansion. saphyr 0.2.2's loader expands each alias by cloning the anchored node with no bound (`src/loader.rs:369-374`), before any conversion runs. Options: refuse any alias (a problem naming the line, like merge keys), or bound the loader's output by pre-scanning for anchors and aliases, or parse with a different front end. Which?

**R3. "No secret appears in a problem" does not hold under `flatten` and `untagged`.** A failing `Secret` there fails inside serde's buffered content, where the deserializer knows no secret path, and serde's message quotes the value it found. The collector records only successful values. Options: state the restriction (those two attributes are unsupported around a `Secret`, and the library says so at the type), or run a second pass that re-renders any problem text against every collected value of the section. Which?

**R4. Diesel keeps diesel-async's default `SpawnBlocking`, which falls back to a hidden runtime when no tokio runtime is current** (`PROBES_db.md` §3.2, case 3). That contradicts the probe's recommendation, `with_runtime(conn, Given(..))`. H2 relied on the name check, which proves the app's runtime is tokio but not that the thread running a query has a tokio context. Accept, or supply a `SpawnBlocking` type that enters the given runtime?

**R5. H6's "smol features are harmless".** A `PoolConnection` dropped on a thread with no tokio context still spawns on smol's global executor (`PROBES_db.md` §1.2). Kept as a premise the stage-2 test checks. Accept that the test decides, with the ruling revised if it fails?

## Open signatures the spine freezes

**S1. A second retry loop.** H1 gives `ulo-db` a loop around `create` beside the core's readiness loop, against the one-copy wording. Two things are left: does the `create` loop's time count against `.ready`'s bound (one total bound, or two), and the factory's construction bound (`construct_timeout`, 30 s by default) could cut the loop off. Proposed: one total bound `connect_within`, the `create` loop and `.ready` share it by passing the remaining time, and `construct_timeout` is set to the same bound when `CREATE_CONNECTS` is true. Confirm.

**S2. `mongodb+srv` in `create`.** The SRV lookup is network I/O in `create`, but MongoDB's `CREATE_CONNECTS` is false, so a DNS failure is never retried. Set `CREATE_CONNECTS` true for a `+srv` URL, or make it true for the store?

**S3. `prefer_tokio()` is a `OnceLock`.** If another crate set a preference first, the call returns an error and that preference stands. What does the module do: log a `warn` and continue, or refuse at `connect` naming the conflict?

**S4. gRPC writes on change need a stored copy.** tonic-health 0.14.6's `HealthReporter` has no public way to read a status. `GrpcHealth` keeps the last status written per name itself, which means one writer per name holds the truth. Confirm that a write through `Dep<GrpcHealth>` by the application also goes through that copy.

**S5. Trace context.**
- "Forwarded exactly as received" (H4) means `TraceContext` keeps the raw header text, `traceparent` and `tracestate` both. Confirm.
- The client needs to tell a forwarded context from one originated here, to apply the `tracestate` rule. Proposed: `TraceContext` carries an `origin` marker, `Received` or `Originated`, set by `ExecOptions::trace` and `set_trace`.
- `span::call` is synchronous and `TraceSource` is bound in DI, whose lookups are async. Proposed: `App` resolves the `TraceSource` once at `connect` and every hub reads it from `AppHandle` (a plain accessor, no await), the way `AppHandle::timer()` is read.
- A standalone execution (no transport) has no `span::call`. Where is `TraceSource::start` called for it: inside `Execution::open`, or not at all?

## For the record

**N1.** The Diesel floor is 1.89 (for `File::lock`); P1 said "Diesel's own floor where higher", but diesel declares 1.86 and diesel-async 1.84. The design states 1.89 on `ulo-db-diesel` itself.

**N2.** ureq 3.4.2's rustls client takes an explicit provider through `TlsConfig::unversioned_rustls_crypto_provider` and never calls rustls's own resolution, so P8's premise holds. The setter is outside ureq's semver guarantee; the crate pins `ureq` exactly.

**N3.** The migration lock key is `K = 3490428366386280370` (`0x30707e1b2c66b7b2`). SQLx's own key is `0x3d32ad9e` times the CRC-32 of the database name, and `K` is not a multiple of `0x3d32ad9e`, so no database name makes the two collide.
