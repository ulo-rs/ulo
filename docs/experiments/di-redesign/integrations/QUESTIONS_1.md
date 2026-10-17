# Questions on the second response (for the designer)

Written for: the designer. Answers a review of the second response against the repository at `2f3c9ed0`. Most premises hold; the questions below are the ones the code or a settled decision does not answer.

Sections worth pasting alongside: core `DESIGN.md` §2, §3.1–3.2, §3.6, §3.9, §4 (the `try_value` paragraph), §8.3, §8.6, §9.1–9.5, §10.2, §13 and decisions 4, 13, 24, 25; transports `DESIGN.md` §2.9, §3.6 "Shutdown", §5.2 (reserved headers), §5.4 (`usable`), §6.2 (Health), §8 ("The app", "A runtime is given, never found", the `Smol::block_on` paragraph, "Two levels"), §10, §11 rows X13/X14/X33/X34, and decisions 46, 124 and 135; the header of `runtime-levels.txt`.

**Q1. The in-task `Handle::try_current()` probe.** Transports decision 46 names `Handle::try_current` as the hidden coupling the rule exists to remove, and `Smol::block_on` runs the executor on the calling thread (§8). A smol app driven from a thread with a tokio context therefore sees `try_current()` succeed on the task's first poll, and the library's own spawns later panic on the smol workers. Do you keep section 2.4's probe, or follow X33/X34: `SqlxModule::new(..)` captures `Tokio::try_current()` where it is built in `main`, or takes `.with_handle(h)`, and refuses in its init hook? A third option: `Runtime` gains `fn kind(&self) -> RuntimeKind`, forwarded by the core's wrapper, so the question is answered without a downcast and without a probe.

**Q2. gRPC health.** Transports §6.2 says health "stays SERVING through the before-shutdown stage, consistent with `is_draining()`". Today the server writes at bind and at drain, and the application writes through `Dep<GrpcHealth>` at any time, so two writers already exist. Your 2.2 flips to NOT_SERVING at `Stopping` and keeps the application's writes.
- (a) Is that a decision change, to be recorded as one?
- (b) Who is the one writer when `Readiness` is bound and the application still calls `set_named`?
- (c) `ulo-grpc` depending on `ulo-health` puts a transport below an integration. Should `Readiness` live in core or `ulo-transport`, or should health write through `Option<Dep<GrpcHealth>>` with `bind` told not to overwrite?

**Q3. E1 (`Secret` and serde).** `Secret` has no serde impl and `ulo` no serde dependency. The config crate owns the deserializer over its origin-carrying tree, so if `Secret`'s `Deserialize` calls `deserialize_newtype_struct("Secret", ..)`, that deserializer can record each secret leaf itself, with no thread-local and no `fw::secret::collect`. Does E1 shrink to a `serde` feature on `ulo` carrying that impl?

**Q4. `ulo-db`'s error types.** With the retry loop replaced by `.ready` and E4, restate the `Store` trait and its errors. `connect(o, rt: &Given)` no longer has a `Given`. `ConnectError { Unreachable, Misconfigured }` clashes with `ulo::ConnectError` (core §10.2) and with `ulo_rpc::Misconfigured` (`crates/ulo-rpc/src/link.rs:435`). What are the names and shapes of `Permanent`, `UnsupportedRuntime` and `MigrationFailed`, and how does a store mark a readiness error permanent?

**Q5. Trace context entry points.**
- How does a standalone `execute` seed an inbound context (`ExecOptions::trace(..)`)?
- What is the mode on an `RpcClient::new(link, runtime)` built outside any `AppBuilder`?
- Is reserving all of `tracestate` intended? W3C Trace Context §3.3 lets a vendor add its own entry.

**Q6. `.lag` and hook ordering.** Before-shutdown hooks run one after another under `TIMEOUT` (10 s default) and the shutdown cap (§9.5), in the exact reverse of connect order with lazily loaded modules first. State `.lag`'s `TIMEOUT` rule, and change "runs among the first" to "before every `Check`'s hooks". Kubelet stops liveness and startup probes at termination and keeps readiness (`prober_manager.go`, `StopLivenessAndStartup`), and marks endpoints terminating at the same time as SIGTERM. Does that change anything in 2.2?

**Q7. Zero rule.** A `Duration` zero is accepted and `Bound::After(ZERO)` refused (transports §3.6). Is the gRPC re-evaluation interval a `Bound`?

**Q8. Lazily loaded modules.** A lazily loaded module cannot contribute to a collection a pre-existing binding reads as `Many` (`LoadRefusal::Contribution`, core §8.6). A lazily loaded database module's check would never reach readiness. Accept this as a stated limit, or ask the core for something?

**Q9. The OpenTelemetry flush.** The `.module()` flush joins a thread inside an async hook, which blocks a worker. Confirm the thread signals completion through a channel the hook awaits under its bound.

**Q10. Levels.** Confirm: every `ulo-db-*` crate is `tokio-bound` (smol deferred); `ulo-db-conformance` is shaped like `ulo-rpc-conformance` (runtime-free with a `tokio` harness feature) or `tokio-bound`; `ulo-otel-otlp` is `tokio-bound`. Withdraw "`runtime-levels.txt` needs this level named": `tokio-bound` is already the fourth name.

## Facts the review found that correct the design

- Destroy hooks run after the drain but before the transports' socket `close`, so `pool.close()` can run under a query an abandoned execution still holds (open premise 7 is the right question).
- The F404 deferral is the one this race closes, not F400.
- Not every RPC frame kind carries `h`: only `Req`, `Evt` and `Open`. That suffices for request-side propagation.
- A failing configuration source yields one `ValueFailed` per module that read a section, and the error's `Display` must carry every problem, since `Redacted` flattens `source()` into one string.
- The scope is large for one race. Suggested staging: configuration and `ulo-health` with HTTP; then `ulo-db` over `.ready` with sqlx and its suite; then the other stores; the CLI templates; trace propagation and `ulo-otel` last.
