# Divergences: race 3, area O2 (`ulo-otel`)

Every place O2 departs from `integrations/DESIGN.md`, `BUILD_PLAN_3.md` or the fourth and fifth
responses, fills a shape they leave open, or resolves a point the plan leaves to O2. All await the
user's sign-off.

No cargo was run. Every library fact below is read from the registry sources under
`~/.cargo/registry/src` (opentelemetry 0.33.1, opentelemetry_sdk 0.33.1, tracing-opentelemetry
0.34.0, futures-channel 0.3.32); what only a compile or a run can show is under "Not verified".

Files written: `crates/ulo-otel/Cargo.toml`, `src/options.rs`, `src/otel.rs`, `src/source.rs`,
`src/sampler.rs`, `src/flush.rs`. `src/lib.rs` and `src/exporter.rs` held no `todo!()` and stay as
S-skel wrote them. No public signature changed. Nothing on `master` is replaced: the crate is new.

## Points the plan leaves to O2

P1. **The flush hook's bound is `After(flush_timeout + 1 s)`.** The flush timeout is
`OtelOptions::flush_timeout_ms`, 5000 when unset, the batch processor's own default
(`opentelemetry_sdk` `trace/provider.rs:273-274`); the flush thread calls
`SdkTracerProvider::shutdown_with_timeout(flush_timeout)`, which returns once the processor has
flushed or that timeout has passed. The hook is bounded one second past it, so a flush that reports
at the processor's own timeout is not cut by the hook a moment earlier; the default hook bound is
6 s. The second is the margin H1 gives `.lag`'s hook. `Duration::saturating_add` keeps a huge
setting from overflowing the addition.

A written bound needs a `Timer` (`crates/ulo/src/graph/wire.rs:948-999`), so an app with no `Timer`
importing `OtelModule` is refused at `wire()` with `BoundWithoutTimer` naming the hook. Leaving the
hook at `Bound::Default` would avoid that, but its 10 s `hook_timeout` would cut a flush timeout
set above it, and the design asks for the hook to await "under its bound". The module's docs state
the requirement.

P2. **How `OTEL_*` names map onto `OtelOptions` (`from_env`).**

| Variable | Field | Rule |
|---|---|---|
| `OTEL_SERVICE_NAME` | `service_name` | as given |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | `endpoint` | as given: a base URL, the exporter appending `/v1/traces` (O3's `lib.rs` states the same reading) |
| `OTEL_EXPORTER_OTLP_HEADERS` | `headers` | a `Secret`; off the `key=value,..` grammar a problem whose message names the shape and never the text |
| `OTEL_TRACES_SAMPLER` | `sample_ratio` | `parentbased_always_on` or unset: `None` (1.0); `parentbased_always_off`: `0.0`; `parentbased_traceidratio`: the argument below; any other value a problem |
| `OTEL_TRACES_SAMPLER_ARG` | `sample_ratio` | read only under `parentbased_traceidratio`, 1.0 when unset; not a number from 0 to 1 a problem |
| none | `flush_timeout_ms` | no standard variable names a shutdown timeout; `OTEL_BSP_EXPORT_TIMEOUT` bounds one export, not the flush |

A variable set to the empty string counts as unset and the sampler's name is matched without regard
to case, both as OpenTelemetry's specification of the variables has it. Each problem's path is the
variable's name.

`always_on`, `always_off` and `traceidratio` are refused rather than read as their parent-based
forms: the design makes sampling parent-based (§6.4), and `always_on` read as
`parentbased_always_on` would drop a trace whose remote parent is unsampled, which the operator who
wrote `always_on` asked not to happen. `jaeger_remote`, `parentbased_jaeger_remote` and `xray` are
refused the same way. Not read: the per-signal names (`OTEL_EXPORTER_OTLP_TRACES_ENDPOINT` is a full
URL where `endpoint` is a base, and the field has one meaning) and `OTEL_SDK_DISABLED`, which no
field carries.

## Entries

O2-1. **`Otel::new` refuses options no reader can mean.** The design is silent. A `sample_ratio`
that is NaN or outside 0 to 1, a `flush_timeout_ms` of zero, and `headers` off the `key=value`
grammar are each a problem at the field's name, returned together as `ulo_config::Problems` inside
the `BoxError`, before the exporter is built. `from_env` refuses the ratio and the headers at the
variable's name already; `from_config` cannot, since `ConfigError`'s fields are `ulo-config`'s, so
the refusal sits where both paths meet. Principle 2 has a misconfiguration refused at the first
phase that can see it, and `main` is that phase for `Otel`. A zero flush timeout is refused because
it would drop every queued span at shutdown with no sign but the `TimedOut`.

O2-2. **An absent inbound context leaves the span's parent to `tracing`.** The skeleton's
`OtelSource` doc read "An absent or invalid inbound context starts a new trace". `start` calls
`set_parent` only for an inbound context; with none, the span keeps the parent `tracing-opentelemetry`
gives it: the span it was created inside, or the explicit parent its creator named, and with neither,
a root of a new trace. Forcing a root with `set_parent(Context::new())` would cut the parent a
standalone execution's author chose when creating the span handed in through `ExecOptions::span`,
whose purpose (I16) is to trace the job where its author put it. `span::call`'s is the only span the
core, `ulo-transport` and the four hubs create (read: no other `*_span!` in their sources), so a
request with no context starts a new trace unless the application entered a span of its own around
the server, in which case the request nests under it. The doc now
says so; the transport hands an invalid context in as absent (H4), so the two cases are one.

O2-3. **What `start` answers when the span has no OpenTelemetry span.** `TraceSource::start` must
answer a `TraceContext`. `span.context()` answers an empty context when no `Otel::layer` sits in the
subscriber, when a filter disabled the span, or after the provider is shut down
(`tracing-opentelemetry` `span_ext.rs:342-357`; `opentelemetry_sdk` `trace/tracer.rs:176-184`).
Nothing was recorded here, so the answer is the inbound context unchanged (its IDs, the next hop
then a child of the caller's span; `set_trace` marks it `Originated`), or with none a new trace from
the SDK's `RandomIdGenerator` with the sampled flag clear: a sampled flag would tell the next hop
this process recorded a span it did not. A warning is logged once per process when the span was
enabled, the subscriber took it, and the flush has not begun: the application installed no layer,
or wrapped it in a `reload` layer, through which `set_parent` cannot reach it (`span_ext.rs:228-246`).

O2-4. **`Otel` and `OtelSource` carry a shared `flushing: Arc<AtomicBool>`.** Private fields, which
rule 4 allows. The flush hook sets it before shutting the provider down, and the source reads it to
keep the warning of O2-3 from firing on a span an `OnApplicationShutdown` hook opens after the
flush, whose empty context is the shut-down tracer's by design. `OtelSource` is no longer a unit
struct; it is `pub(crate)`.

O2-5. **The parent is remote when the inbound context is `Received`.** `SpanContext::new` takes
`is_remote`. A `Received` context came from another process; an `Originated` one is this execution's
first span, which a second `span::call` for the same execution continues (race3-S C14), and is
local. A WebSocket message's inbound context is the connection's own, re-seeded through
`ExecOptions::trace` and so marked `Received` (race3-S C18), and is read as remote though it is
local; the origin is the only signal the core gives, and the flag changes only the exported
`parent_is_remote` and which branch of the parent-based sampler applies, both of which default
alike.

O2-6. **The module is global.** `connect` resolves the source from the root module (race3-S C12).
Global, `OtelModule` is found wherever the application imports it; imported below the root without
it, the source would be absent with no word of why. `ulo-ws`'s `WsModule` is global for the same
reason.

O2-7. **The headers are registered for redaction in `OtelModule::register`, the whole text and each
value.** `from_config` reads through `Section::read` and keeps only the value: `OtelOptions` is not
bound through `Section::register`, so the secrets the section collected would never reach
`m.secret(..)`. The module registers `headers` itself, which covers both readers, and registers each
header's value too, since an exporter's error may print one value without the rest of the text
(P3's measure 1 does the same for a URL and its password). An empty text or value is skipped.

O2-8. **The resource reads no environment variable.** `Resource::builder()` runs
`SdkProvidedResourceDetector` and `EnvResourceDetector`, which read `OTEL_SERVICE_NAME` and
`OTEL_RESOURCE_ATTRIBUTES` from the process environment (`opentelemetry_sdk` `resource/mod.rs:62-70`,
`resource/env.rs`). `from_env` takes a given `Env` so a test fills it from a map; a resource read
from the process beside it would answer what the test did not set. The resource is
`Resource::builder_empty()` with `TelemetryResourceDetector` (`telemetry.sdk.*`) and `service.name`,
the options' or `unknown_service:<executable name>`, the semantic conventions' fallback, written by
`ulo-otel` from `std::env::current_exe`. Limit, stated on `Otel::new`: the SDK's own defaults read
`OTEL_BSP_*` (`trace/span_processor.rs:1069-1225`) and the `OTEL_SPAN_*_LIMIT` names
(`trace/config.rs:29-60`) from the process environment, and `TracerProviderBuilder` offers no
default that skips them; `OTEL_TRACES_SAMPLER`, which `Config::default` also reads, is overridden by
`with_sampler`.

O2-9. **Sampling is `Sampler::ParentBased(TraceIdRatioBased(ratio))`, `AlwaysOn` at the root when
the ratio is unset.** `sampler`'s frozen signature answers a `Sampler`, so the enum variant, not
`ParentBasedSampler`. The variant's four parent branches are the specification's defaults: a sampled
parent sampled, an unsampled parent dropped, remote or local (`trace/sampler.rs:195-240`).
`TraceIdRatioBased` answers `RecordAndSample` at 1.0 and above and compares the trace ID below it
(`trace/sampler.rs:400-418`).

O2-10. **The layer's tracer reports the instrumentation scope `ulo-otel`, with the crate's
version.** The design is silent. The spans are `ulo-transport`'s, but the scope names what produced
the OpenTelemetry spans, which is this crate's layer; `opentelemetry_sdk` logs an empty name.

O2-11. **The flush thread's outcome is logged, and a failed spawn travels on a second oneshot.** The
hook returns `()`, so a flush error (`AlreadyShutdown`, a timeout, an export failure) is a `warn`
naming it. `thread::Builder::spawn` drops its closure, and the sender in it, when it fails, which
would leave the receiver answering only `Canceled`; `spawn` returns a second receiver already holding
the spawn error. The thread is named `ulo-otel.flush`. A second call of the hook spawns a second
thread whose shutdown answers `AlreadyShutdown`.

O2-12. **`opentelemetry` names its `trace` feature in the manifest.** The plan writes "`opentelemetry`
with no default features"; the workspace entry has none, and `trace` (`futures`, `thiserror`) is
what `SpanContext`, `TraceState` and the ID types sit behind. `opentelemetry_sdk`'s `trace` enables
it as well, so the tree is unchanged; naming it keeps the crate from depending on that. No feature
reaches tokio (`opentelemetry-0.33.1/Cargo.toml`, `[features]`).

O2-13. **`OtelOptions`'s field docs changed, its fields did not.** `endpoint` is documented as the
receiver's base URL, the reading O3's `lib.rs` gives it, `service_name` with its fallback,
`sample_ratio` with its range and the sampler name it is read under, `flush_timeout_ms` with its
default, its zero refusal and the hook's margin.

## Requests to other owners

None.

## Read in other areas' files, edited nothing

- `crates/ulo-otel-otlp/src/lib.rs` (O3) documents that, with no `endpoint`, "the exporter reads the
  standard `OTEL_*` variables itself": `opentelemetry-otlp`'s `resolve_http_endpoint` reads
  `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT` and `OTEL_EXPORTER_OTLP_ENDPOINT` from the process
  environment (`exporter/http/mod.rs:762-795`). An application whose `OtelOptions` came from
  `from_env` over a map then still has its endpoint decided by the process environment when the map
  names none. O2-8's reasoning applies; whether O3 passes an explicit default endpoint is O3's to
  decide.

## Not verified

- That the crate compiles: no cargo was run. The edition-2024 let-chains are stable at the 1.88
  floor and used in the core.
- That `OpenTelemetryLayer::new`'s bound `SdkTracer::Span: Send + Sync` holds; PROBES_obs.md §2's
  probe built the same layer over `SdkTracerProvider`.
- That a `Fn(Arc<OtelSource>) -> Arc<dyn TraceSource>` closure written `|source| source` coerces at
  its return, as `ulo-ws`'s `also_as::<dyn BroadcastAdapter>(|adapter| adapter)` relies on.
- The `AlreadyStarted` branch: the core calls `start` before the span is entered, so it is reached
  only if a transport enters the span first; it is logged at `debug` and the span keeps its own
  parent.
- A `flush_timeout_ms` near `u64::MAX`: the addition saturates, but the core's `Timer` receives the
  bound, and whether a runtime's sleep accepts it is the runtime's.
- Every runtime behaviour: the trace scenarios of §6.5 with a recording tracer, the flush's
  `TimedOut` on an exporter that stalls, and the warning of O2-3, are the coordinator's tests.
