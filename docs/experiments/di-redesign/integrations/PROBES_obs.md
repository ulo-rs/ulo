# Probes: health and observability premises (race 3)

Probe batch for RESPONSE.md second response sections 2.2 and 2.5 and third response Q2, Q5, Q6, Q7 and Q9, and CAPABILITIES.md capabilities 6 to 10 and 26.

Claim tags: **Probed** (code run, output below), **Read** (source file and line), **Spec** (standard text), **Unverified**.

Environment: rustc and cargo 1.98.1, macOS arm64, repo HEAD `58e7637b`. Scratch crates live under `/private/tmp/claude-501/-Users-ifeanyi-Projects-Personal-open-source-toni-rs-toni-rs/a632688a-d566-4ff7-b727-8f28e1dd90c6/scratchpad/race3-probes-obs/`, one crate per probe: `p1-health`, `p2-parent`, `p3-notokio`, `p3b-deadlock`, `p4-otlp`, `p4b-minimal`, `p4c-ownclient`, `p5-w3c`. Nothing in the repo was touched except this file.

Versions: tonic 0.14.6, tonic-health 0.14.6 (both the repo lock's), tokio 1.53, opentelemetry 0.33.1, opentelemetry_sdk 0.33.1, opentelemetry-otlp 0.33.1, opentelemetry-proto 0.33.1, tracing-opentelemetry 0.34.0 (latest on crates.io today for all five), tracing 0.1.44, tracing-subscriber 0.3.23, reqwest 0.13.5, ureq 3.4.2.

---

## 1. tonic-health 0.14.6 against `grpc.health.v1`

Probe: `p1-health/src/main.rs`. A tonic server with `health_reporter()` on a loopback port and a tonic client (`HealthClient`) calling `Check` and `Watch`.

Command: `cargo run -q`

Raw output:

```
[a] Check("") on fresh reporter = Serving
[a2] Check("nope") unregistered = Err code NotFound msg "service not registered"
[b] Watch(svc) initial = Serving
[b] Watch(svc) after set NOT_SERVING = NotServing after 103.542µs
[b2] same-value write -> message within 500ms? true
[a3] Watch("") initial=Serving after set=NotServing
[a3] Check("") after set NOT_SERVING = NotServing
[c] Watch(late) unregistered = Err code NotFound msg "service not registered"
[d] after A writes ""=SERVING and B writes x.Svc=NOT_SERVING: Check("")=Serving Check(x.Svc)=NotServing
[d] after B rewrites x.Svc: Check("")=Serving (unchanged by named writes)
[e] open Watch(svc) after clear_service_status: Ok(None)
```

Spec text (`grpc-proto/grpc/health/v1/health.proto` and `grpc/doc/health-checking.md`, master):

- Check: "If the service name is not registered, the server returns a `NOT_FOUND` GRPC status." The empty string is the overall server health.
- Watch: "The server will immediately send back a message indicating the current serving status. It will then subsequently send a new message whenever the service's serving status changes."
- Watch, unknown service: "If the requested service is unknown when the call is received, the server will send a message setting the serving status to SERVICE_UNKNOWN but will *not* terminate the call. If at some future point, the serving status of the service becomes known, the server will send a new message with the service's serving status."

Results per question:

| Question | Result | Tag |
| --- | --- | --- |
| `set_service_status` notifies `Watch` immediately | Yes, 104 microseconds from write to message on loopback. The status sits in a `tokio::sync::watch` channel (`tonic-health-0.14.6/src/server.rs`, `set_service_status`). | Probed |
| `Watch` sends the initial status on subscribe | Yes. | Probed |
| `Watch` sends an update on change | Yes. | Probed |
| `Watch` on an unregistered service | Does not hold. The call fails at once with `NOT_FOUND` ("service not registered"); the spec wants a `SERVICE_UNKNOWN` message and an open call. `server.rs` `watch()` returns `Status::not_found` when the name is missing, and the crate's `ServingStatus` enum has only `Unknown`, `Serving`, `NotServing`, so it cannot emit `SERVICE_UNKNOWN` at all. | Probed, Read |
| `Check("")` answers the overall status | Yes. A fresh `HealthReporter::new()` holds `""` as SERVING, and a write to `""` changes both `Check` and `Watch("")`. | Probed, Read (`server.rs`, `HealthReporter::new`) |
| `Check` on an unregistered name | `NOT_FOUND`, as the spec says. | Probed |
| Same-value write | Sends a duplicate `Watch` message ([b2]: a second `NotServing` write over `NotServing` produced a message within 500 ms). `watch::Sender::send` notifies on every write, and `set_service_status` does not compare. | Probed |
| `clear_service_status` | Ends open `Watch` streams with `Ok(None)` (a clean end), which the spec tells a client to treat as retry-with-backoff. | Probed |

Writers, today (Read):

- `crates/ulo-grpc/src/health.rs:21-24`: `GrpcHealth::new` is `HealthReporter::new()`, so `""` starts SERVING without any write.
- `crates/ulo-grpc/src/server.rs:385-387` (`bind`): writes SERVING for every handler-derived service name. It does not write `""`.
- `crates/ulo-grpc/src/server.rs:405-409` (`drain`): writes `""` NOT_SERVING, then each named service NOT_SERVING.
- `crates/ulo-grpc/src/health.rs:36-44`: `set` and `set_named`, the application's writer for named services.

Can `""` have one writer and named services another? Yes at the data level ([d]): the reporter is a map of independent `watch` channels, and a clone of the reporter written from two places does not interfere across keys. There is no ownership check inside tonic-health, so "one writer per key" is a convention the design enforces by what it calls, and the last write to a key wins regardless of who made it.

What it means for the design:

1. Q2(c)'s `GrpcHealth::delegate_overall()` is sufficient for `""`: after it, `server.rs:406` stops writing `""` and the bridge is the sole writer. Nothing in tonic-health conflicts with that split.
2. The 5 s re-evaluation (section 2.2, Q7) must not write blindly. An unchanged value still emits a `Watch` message to every watcher. The bridge has to keep the last value it wrote per key and write only on change, or every watcher gets a message every 5 s.
3. `Watch` on a name the app never registered fails with `NOT_FOUND` instead of `SERVICE_UNKNOWN`. Section 3 of the first response says the probe checks that `Watch` "behaves as `grpc.health.v1` specifies"; it does not for this case. Kubernetes' gRPC probe uses `Check`, so the cost is limited to other `Watch` clients. The proto says a client retries with backoff on any status but `UNIMPLEMENTED`, so they would back off instead of seeing `SERVICE_UNKNOWN` (Spec; no such client was run, Unverified). Options: accept and document the deviation, or serve a thin `Health` implementation of our own over the same map. Recommend documenting for now, because tonic-health is the maintained piece and the deviation has no effect on the Kubernetes probe.
4. Race on the drain: readiness flips at `Stopping` (2.2) and the server's `drain` writes NOT_SERVING at `Draining`. With the split, `""` has one writer (the bridge) and the order is by construction. For named services both the server and `set_named` still write, last write wins (Q2(b) already records this as deferred).

Verdict: **partly**. Immediate notification, initial message, update message and `Check("")` hold. `Watch` of an unregistered service does not.

---

## 2. tracing-opentelemetry: parenting a `tracing` span on an extracted context

Latest tracing-opentelemetry is 0.34.0, which depends on `opentelemetry` 0.33.0 with only the `trace` feature (and optional `metrics`); it has no dependency on `opentelemetry_sdk` and none on tokio (`cargo tree -i tokio` in `p2-parent` prints `error: package ID specification 'tokio' did not match any packages`). Compatible set: `opentelemetry` 0.33.1, `opentelemetry_sdk` 0.33.1, `opentelemetry-otlp` 0.33.1, `tracing-opentelemetry` 0.34.0. (Read: `tracing-opentelemetry-0.34.0/Cargo.toml`.)

Probe: `p2-parent/src/main.rs`. A subscriber of `Registry` plus `tracing_opentelemetry::layer()` over an `SdkTracerProvider` with a recording `SpanExporter` and `SimpleSpanProcessor`. The span is built like `ulo_transport::span::call` (`info_span!("call", otel.name = name, ulo.transport = .., http.route = Empty, ..)`). The parent is an `opentelemetry::Context` built from a parsed `traceparent` with `Context::new().with_remote_span_context(SpanContext::new(.., is_remote = true, TraceState))`, with no propagator involved.

Command: `cargo run -q --release`

Raw output:

```
upstream: trace_id=4bf92f3577b34da6a3ce929d0e0e4736 parent span_id=00f067aa0ba902b7 flags=01 tracestate=congo=t61rcWkgMzE
[1] set_parent right after creation -> Ok(())
[1] span.context() after set_parent: trace_id=4bf92f3577b34da6a3ce929d0e0e4736 span_id=6241b4b4ebfc9453 flags=01 tracestate="congo=t61rcWkgMzE"
[1] exported: name="GET /users/{id}" trace_id=4bf92f3577b34da6a3ce929d0e0e4736 span_id=6241b4b4ebfc9453 parent_span_id=00f067aa0ba902b7 parent_is_remote=true flags=01 tracestate="congo=t61rcWkgMzE"
[2] set_parent after enter -> Err(AlreadyStarted)
[2] exported: name="late" trace_id=69e83db677528a6944b8a7c6d447ac66 span_id=28e6d7b6c9a385d7 parent_span_id=0000000000000000 parent_is_remote=false flags=01 tracestate=""
[3] exported: name="root" trace_id=f845749f51c24fe82f5fddda4dcbf50a span_id=b5a0ec10789c4ef8 parent_span_id=0000000000000000 parent_is_remote=false flags=01 tracestate=""
[4] remote parent flags=00, ParentBased(AlwaysOn): exported count = 0
[5] exported: name="child" trace_id=4bf92f3577b34da6a3ce929d0e0e4736 span_id=646bcc6021d31930 parent_span_id=9444f633588960d4 parent_is_remote=false flags=01 tracestate=""
[5] exported: name="server" trace_id=4bf92f3577b34da6a3ce929d0e0e4736 span_id=9444f633588960d4 parent_span_id=00f067aa0ba902b7 parent_is_remote=true flags=01 tracestate=""
[6] 200000 spans (AlwaysOff sampler): without set_parent 214.061541ms (1.07µs/span), with 299.983084ms (1.499µs/span)
```

Findings:

| Claim | Result | Tag |
| --- | --- | --- |
| `OpenTelemetrySpanExt::set_parent` immediately after creation sets the exported parent | Yes: exported `trace_id` is the upstream one, `parent_span_id` is the upstream span id, `parent_is_remote=true`, `tracestate` carried onto the span. The span's own id is new. | Probed |
| It must happen before the span is first entered or `.context()` is called | Yes. After `enter`, `set_parent` returns `Err(AlreadyStarted)` and the span exports as a new root. Source: the span's OTel builder is consumed on first activation (`span_ext.rs`, `OtelDataState::Builder` / `Context`). | Probed, Read |
| `span.context()` after `set_parent` gives the span's own ids for injection | Yes: same trace id, fresh span id, upstream tracestate, flags kept ([1]). This is what a `TraceSource` would read to give the execution its own span's ids. Calling it starts the span, so it must follow `set_parent`. | Probed |
| Sampling honors the remote flags | Yes: with `ParentBased(AlwaysOn)` a remote parent with flags `00` produced no exported span ([4]). | Probed |
| Children opened inside the entered server span parent on it | Yes ([5]): child's `parent_span_id` is the server span's id, same trace id. | Probed |
| A custom layer can set the parent from a recorded field | No, not through public API. `OtelData` and `WithContext` are `pub(crate)` (`tracing-opentelemetry-0.34.0/src/lib.rs:156`, `layer.rs:121`), and `OpenTelemetrySpanExt` is implemented only on `tracing::Span`, which a layer in `on_new_span` does not hold. The documented field hooks are `otel.name`, `otel.kind`, `otel.status_code`, `otel.status_description` and no parent field. | Read |
| Cost | `set_parent` adds about 0.43 microseconds per span in release mode with the sampler dropping everything (1.07 to 1.50 microseconds per span including create and enter). | Probed |

Recording `otel.kind` after creation is supported while the builder is unconsumed: `on_record` updates the builder (`layer.rs:1272-1293`). `span::call` (`crates/ulo-transport/src/span.rs:35-54`) declares no `otel.kind` or `otel.status_code` field today; adding `otel.kind = Empty` and `otel.status_code = Empty` there is a transport change that the section 2.5 plan needs for SERVER kind and the 5xx rule. Read, not run.

What it means for the design:

1. The parent must be set by code that holds the `tracing::Span`, between creation and the first poll of `.instrument(span)`. In every transport the order is already: open the execution, create the span (`span::call`), `.instrument(call_span.clone())` (HTTP `service.rs:176-179`, gRPC `dispatch.rs:169-185`, RPC `dispatch.rs:662`, WS `connection.rs:838` and `1401`). The hook goes between creation and `instrument`.
2. Because the transport crates must not depend on OpenTelemetry, the hook is a trait object in the core: a `TraceSource` (E3) with a method taking `&tracing::Span` and the execution's `TraceContext`; `ulo-otel` implements it by converting to an `opentelemetry::Context` and calling `set_parent`, then `span.context()` to read the span's ids back into the execution. A field-based layer, the other option in the brief, is not available. Without `ulo-otel`, nothing calls `set_parent`, and pass-through still works because the execution holds the incoming `TraceContext` and `.within` forwards it.
3. `ulo-otel` takes `tracing-opentelemetry` as a dependency, which is runtime-free (no tokio) at 0.34.0.

Verdict: **holds**, with the mechanism fixed as `set_parent` through a core-owned hook, not a field-reading layer.

---

## 3. `opentelemetry_sdk` without `rt-*`: tokio in the tree, batch processor thread, shutdown

### 3a. Tree

`p3-notokio/Cargo.toml` dependencies:

```toml
opentelemetry = { version = "0.33.1", default-features = false }
opentelemetry_sdk = { version = "0.33.1", default-features = false, features = ["trace"] }
```

`opentelemetry_sdk`'s `trace` feature enables `opentelemetry/trace`, `rand`, `percent-encoding`, `futures-channel`, `futures-executor`, `futures-util`, `thiserror`. The `rt-tokio` and `rt-tokio-current-thread` features and `experimental_async_runtime` are the only ones that reach tokio (`experimental_trace_batch_span_processor_with_async_runtime` adds `tokio/sync`; `testing` adds `tokio/sync`, which also pulls `InMemorySpanExporter`'s feature gate; avoid `testing` in a runtime-free crate and write the 15-line recording exporter used in these probes).

```
$ cargo tree -i tokio
error: package ID specification `tokio` did not match any packages
```

34 unique normal-dependency crates, none of them tokio. `tracing-opentelemetry` 0.34.0 added on top (p2-parent) is also free of tokio. Probed.

`cargo tree -i tokio` prints an error rather than an empty list when the crate is absent; a CI check should assert on that exit status (non-zero) and on the message, not on empty output.

### 3b. The batch processor exports from its own thread

Probe `p3-notokio/src/main.rs`: a `BatchSpanProcessor::builder(exporter).build()` with a recording exporter that notes `std::thread::current().name()` in `export`.

```
main thread = Some("main")
[1] shutdown result=Ok(()) in 99.584µs; exported on: [("span0", "OpenTelemetry.Traces.BatchProcessor"), ("span1", "OpenTelemetry.Traces.BatchProcessor"), ("span2", "OpenTelemetry.Traces.BatchProcessor")]
```

The processor owns a thread named `OpenTelemetry.Traces.BatchProcessor`; the exporter's future is run there (the sdk uses `futures_executor::block_on` on that thread). Probed.

### 3c. Does `shutdown` block, and can it deadlock on a current-thread runtime?

Probe `p3b-deadlock/src/main.rs`: a current-thread tokio runtime (tokio features `rt,sync,macros,time` in the caller), three cases on the runtime's only thread.

```
[A] plain exporter, shutdown() on the runtime thread -> Ok(()) in 60.083µs
[B] runtime-needing exporter, shutdown_with_timeout(3s) on the runtime thread -> Err(InternalFailure("Shutdown errors: [Timeout(3s)]")) in 3.005038s
[C] same exporter, shutdown on its own thread, awaited -> Ok(()) in 98.375µs
```

- `shutdown` blocks the calling thread until the batch thread has flushed or the timeout (default 5 s: `provider.rs:273-274`, Read) passes.
- It does not deadlock by itself. Case A: an exporter whose future needs nothing from the runtime completes while the runtime thread is blocked.
- It stalls when the exporter's future needs the blocked runtime. Case B: the export awaits a task that only the current-thread runtime can run, and `shutdown` returns `Timeout` after the whole bound. The stall is bounded by the timeout, not forever, and the spans are lost.
- Case C is the design's shape: calling `shutdown` from a thread of its own and awaiting a oneshot never blocks the worker.

The design's claim ("`shutdown` blocks, and can deadlock if called on the main thread of a current-thread tokio runtime") holds in a narrower form: the stall needs an exporter bound to that runtime. With the OTLP exporter on `reqwest-blocking` or a ureq client (section 4), the exporter does not need the app's runtime, and case A applies (section 4 shows `shutdown` on the runtime thread returning `Ok`). The off-thread flush in Q9 stays correct, and costs little: it is required for any exporter that may be async-runtime-bound, and it also keeps a worker from blocking for up to the shutdown timeout, which the brief's "nothing on a runtime worker blocks" demands regardless of deadlock.

Verdicts: tokio-free tree **holds**; thread-based export **holds**; deadlock **partly** (a bounded stall, only with a runtime-bound exporter; the off-thread flush is still the right design).

---

## 4. `opentelemetry-otlp`: features, tokio, no-runtime export

Defaults of 0.33.1 (`opentelemetry-otlp-0.33.1/Cargo.toml`, Read): `http-proto`, `reqwest-blocking-client`, `trace`, `metrics`, `logs`, `internal-logs`. gRPC is the separate `grpc-tonic` feature, which pulls `tonic` and `tokio`.

### 4a. Default features

```
$ cd p4-otlp && cargo tree -i tokio
tokio v1.53.3
├── hyper v1.12.0
│   ├── hyper-util v0.1.21
│   │   └── reqwest v0.13.5
│   │       ├── opentelemetry-http v0.33.1
│   │       │   └── opentelemetry-otlp v0.33.1
...
```

Tokio is in the tree through reqwest (hyper, hyper-util, tower, tower-http). Narrowing to `default-features = false, features = ["http-proto", "reqwest-blocking-client", "trace"]` (`p4b-minimal`) still shows tokio via the same path (tokio features `net`, `rt`, `sync`, `time`). Probed.

### 4b. Export to a local OTLP HTTP receiver, no tokio in the caller

Probe `p4-otlp`: a `tiny_http` receiver on `127.0.0.1:0` decoding the body as `ExportTraceServiceRequest` with `opentelemetry-proto`, and an `SpanExporter::builder().with_http().with_endpoint(..)` behind a `BatchSpanProcessor`. Three modes.

```
== sync            (plain main, no tokio in the caller)
[sync] shutdown -> Ok(())
receiver got: POST /v1/traces content-type=Some("application/x-protobuf") bytes=238
    span name="probe-span" trace_id=4c4b8a1718642fdc47878779bae502df span_id=f76fdcac5fe67ec5 parent_span_id=
== async-direct     (exporter built and shutdown() called on a current-thread tokio runtime's only thread)
[async-direct] shutdown on runtime thread -> Ok(())
receiver got: POST /v1/traces content-type=Some("application/x-protobuf") bytes=238
    span name="probe-span" trace_id=c6e88811d1d0c1ec994d6e1444849888 span_id=22a9d75938be414e parent_span_id=
== async-build-only (built on a plain thread, shutdown called inside the runtime)
[async-build-only] shutdown inside runtime -> Ok(())
receiver got: POST /v1/traces content-type=Some("application/x-protobuf") bytes=238
    span name="probe-span" trace_id=f0eccd3824f703636437b1caa888dd02 span_id=5d6c0120f92ce2e7 parent_span_id=
```

`http-proto` plus `reqwest-blocking-client` works with no runtime in the caller, and also works when built and shut down on a current-thread runtime's only thread (no "cannot drop a runtime" panic at 0.33.1; the blocking client does its work on the batch thread). Probed.

### 4c. A runtime-free OTLP tree

Because `reqwest-blocking-client` is what brings tokio, `p4c-ownclient` drops it and supplies the `HttpClient` trait (`opentelemetry-http-0.33.1/src/lib.rs:137-147`: one `async fn send_bytes(&self, Request<Bytes>) -> Result<Response<Bytes>, HttpError>`) with a 20-line implementation over `ureq` 3.4.2, blocking inside the async fn on the batch thread. Features: `opentelemetry-otlp = { default-features = false, features = ["http-proto", "trace"] }`, `opentelemetry-http = { default-features = false }`.

```
$ cd p4c-ownclient && cargo tree -i tokio
error: package ID specification `tokio` did not match any packages
$ ./target/debug/p4c-ownclient sync
[sync] shutdown -> Ok(())
receiver got: POST /v1/traces content-type=Some("application/x-protobuf") bytes=244
    span name="probe-span" trace_id=9fd70d2fbc7c4101586f0e2236344223 span_id=4851d32b85e6940e parent_span_id=
```

The exporter-side tree (otlp, http, proto, prost, sdk, futures-*) has no tokio and no tonic (`opentelemetry-proto` with `gen-tonic-messages` is messages only). The ureq client brings rustls for https endpoints. Probed.

What it means for the design:

1. Section 2.5's "`ulo-otel-otlp` goes in `runtime-levels.txt` as tokio-bound" is true for the default `reqwest-blocking-client` and avoidable: `ulo-otel-otlp` can be runtime-free by shipping a small `HttpClient` over a blocking client (ureq), at the price of owning that client (TLS roots, timeouts, proxies). Recommend: default to the `ureq` client and keep a `reqwest-blocking` cargo feature for applications that want the reqwest behavior, with the level recorded per feature. Confidence in the tradeoff is medium; the choice of ureq is an implementation detail the probe shows works, not a vetted recommendation of its TLS stack.
2. The brief's rule "a runtime-free crate has no async runtime anywhere in its tree" is then satisfiable for `ulo-otel` and `ulo-otel-otlp`, which removes the need for the "tokio-bound, on a thread of its own, never touches the app's runtime" caveat.
3. gRPC export stays deferred, as the response says: `grpc-tonic` requires tokio.

Verdict: default features bring tokio (**as the response says**); HTTP/protobuf export without a caller runtime **holds**; the response's conclusion that the crate must be tokio-bound **does not hold** if the HTTP client is supplied.

---

## 5. W3C Trace Context: rules and a reference implementation

Sources read: https://www.w3.org/TR/trace-context/ (W3C Recommendation 23 November 2021, Level 1), https://www.w3.org/TR/trace-context-2/ (Candidate Recommendation Draft 28 March 2024, Level 2), and the W3C conformance harness `https://github.com/w3c/trace-context/blob/main/test/test.py` for behaviours the prose leaves open. Section numbers below are Level 1's. Note: the mutation rules for `tracestate` are section 3.5, not 3.3 as the third response's Q5 cites (3.3 is the header grammar).

### (a) Invalid `traceparent` (Spec 3.2.2, 3.3, 4.3; harness)

- Header name matched case-insensitively; written lowercase (3.2.1).
- ABNF: `version "-" trace-id "-" parent-id "-" trace-flags`, version 2 lowercase hex, trace-id 32 lowercase hex, parent-id 16 lowercase hex, flags 2 lowercase hex. Uppercase hex is invalid ("non-lowercase hex characters", 3.2.2.4).
- Version `ff` is invalid. All-zero trace-id or parent-id is invalid ("MUST ignore the traceparent", 3.2.2.3 and 3.2.2.4).
- Version `00` is exactly 55 characters. Harness: `00-...-01.` and `00-...-01-what-the-future-will-be-like` both restart the trace.
- If the version prefix cannot be parsed (not 2 hex characters followed by a dash), restart the trace (3.2.4).
- Invalid `traceparent`: create a new `traceparent` and delete `tracestate` (4.3). `tracestate` with no `traceparent` is discarded (4.2). "If the vendor failed to parse traceparent, it MUST NOT attempt to parse tracestate", and the reverse does not hold: a bad `tracestate` MUST NOT affect `traceparent` (3.3).
- Not in the prose, from the harness: two `traceparent` header fields restart the trace (`test_traceparent_duplicated`); leading and trailing space or tab around the value is accepted (`test_traceparent_ows_handling`).
- This is a restart, not a failed request, so the third response's contrast with `deadline-ms` holds (Spec).

### (b) Higher version forward compatibility (Spec 3.2.4, 4.3)

- Do not analyze the version in a pass-through service.
- Version above `00` (and not `ff`): if the header is shorter than 55 characters, restart. Else parse trace-id (32 hex, then a dash), parent-id (16 hex, then a dash), and the 2 flag characters, which must be followed by end of string or a dash. If all three parse, use them. The harness shows the case that fails: `cc-...-01.what-the-future-will-be-like` restarts, while `cc-...-01-what-the-future-will-be-like` and plain `cc-...-01` continue the trace.
- Do not parse or assume anything about the unknown trailing fields. The outgoing header is rebuilt as version `00` ("downgrade the version" is the first mutation).
- Outgoing flags: only the bits this implementation knows are kept; the rest are written as zero (3.2.2.5.2 and 4.3). Level 1 knows `sampled` (0x01). Level 2 adds `random` (0x02) and its harness case `test_propagates_random_flag` expects it to survive. The reference implementation keeps 0x03.

### (c) `tracestate` limits and grammar (Spec 3.3.1)

- A list of `key=value` members separated by commas, whitespace (space and tab) around members ignored, empty and whitespace-only members allowed and accepted. Several `tracestate` header fields are combined into one list in field order (RFC 7230).
- At most 32 members. The harness expects 33 members to discard the list (strict level 2); the prose says "maximum of 32".
- Key: `simple-key = lcalpha 0*255(lcalpha / DIGIT / "_" / "-" / "*" / "/")` (1 to 256 characters), or `tenant-id "@" system-id`: tenant 1 to 241 characters starting with a lowercase letter or digit, system 1 to 14 characters starting with a lowercase letter. The harness pins 256 ok and 257 rejected, `t`x241 `@` `v`x14 ok, and `t`x242 `@v` and `t@`+`v`x15 rejected by the prose grammar (the harness in fact accepts those two; the grammar rejects them, and the reference implementation follows the grammar).
- Value: 0 to 256 printable ASCII characters from 0x20 to 0x7E excluding `,` and `=`, and the last character not a space. The harness: `foo=bar=baz` discards the tracestate.
- One entry per key. The prose says the vendor "must overwrite their entry upon reentry"; for a duplicate key received, the harness accepts keeping either. The reference keeps the leftmost.
- Invalid `tracestate`: the vendor MAY discard the whole header or just the bad entries (4.3). The harness expects the whole list discarded at its strict level. The reference discards the whole list.
- Limits when propagating (3.3.1.5): "SHOULD propagate at least 512 characters of a combined header" including commas and optional whitespace. If truncation is needed, remove whole entries only; entries longer than 128 characters first, then from the end of the list.

### (d) Mutation when adding an entry (Spec 3.5, 4.3)

- Order of unmodified pairs is preserved. A new pair is added to the beginning (left). An updated value is moved to the beginning. A key appears once, so adding a key that exists drops the older entry.
- Deleting is allowed, but "SHOULD NOT delete keys that were not generated by them".
- If `traceparent` is passed through unchanged, `tracestate` MUST NOT be modified either (3.4). A service that opens a span has changed `parent-id` and may mutate `tracestate`.
- This is the order the third response's Q5 describes, with the correction that a caller's `tracestate` entries and the execution's entries are both "existing" pairs: the spec adds a new key at the left and moves an updated key left, it does not put "the caller's entries first, then the execution's" as a class. Under Q5's merge, a key present in both keeps the caller's value (it is newer), which agrees with "updated key moves left". Fine as a rule, state it as that.

### Reference implementation

`p5-w3c/src/lib.rs` (std only, no dependencies): `TraceContext { trace_id: [u8;16], parent_id: [u8;8], flags: u8, state: TraceState }`, `TraceContext::extract(&[&str] traceparent, impl IntoIterator<&str> tracestate) -> Option<TraceContext>`, `traceparent()` formatter, `TraceState::{parse, header, with_entry}`. 16 tests: the spec's own examples (3.2.3 both traceparent examples; 3.1's Congo/Rojo chain building `rojo=00f067aa0ba902b7,congo=t61rcWkgMzE` then `congo=ucfJifl5GOE,rojo=00f067aa0ba902b7`) and the harness vectors above.

```
$ cargo test -q
running 16 tests
test result: ok. 16 passed; 0 failed
```

Negative control (the check run against a planted break): replacing the lowercase-hex test with `is_ascii_hexdigit` and disabling the version-`00` length check made `invalid_traceparent_restarts_and_drops_state` fail (`15 passed; 1 failed`); restoring brought it back to 16 passed. Probed.

Rules a runtime-free `TraceContext` value must implement (summary): lowercase-hex 55-character `00` grammar; higher-version prefix parse by the same offsets with end-or-dash after the flags; `ff` and all-zero ids invalid; duplicate or missing `traceparent` means restart; OWS trimmed around the value; flags kept as bits with unknown bits zeroed on output; `tracestate` parsed only when `traceparent` is valid, multiple fields joined, at most 32 members, key and value grammar above, any invalid member discards the list and never affects `traceparent`; `with_entry` puts the key leftmost, drops the older same-key entry, truncates to 32 members and 512 characters by whole entries (over 128 characters first, then from the right, never the entry just added); output always version `00`. The value needs no clock and no randomness (id generation belongs to the tracer, not the value).

Open decision for the design: Level 1 (Recommendation) or Level 2 (Candidate, adds `random` flag, allows dropping duplicate keys). The reference keeps 0x03, which satisfies both for received contexts. Unverified: whether the existing `ulo-otel` span ids set the random flag when it generates ids (the OTel SDK's default id generator is random for all 16 bytes, so setting the flag would be accurate).

---

## 6. Kubernetes probe behavior at termination

Source: kubernetes/kubernetes at commit `98d0129cf21ce0871637ae36e67b5ff82d3db50d` (master HEAD when read), `https://github.com/kubernetes/kubernetes/blob/98d0129cf21ce0871637ae36e67b5ff82d3db50d/pkg/kubelet/prober/prober_manager.go`.

Liveness and startup stop at termination (Read):

`prober_manager.go:80-81`:
```go
	// StopLivenessAndStartup handles stopping liveness and startup probes during termination.
	StopLivenessAndStartup(pod *v1.Pod)
```
`prober_manager.go:244-258`:
```go
func (m *manager) StopLivenessAndStartup(pod *v1.Pod) {
	...
		for _, probeType := range [...]probeType{liveness, startup} {
			key.probeType = probeType
			if worker, ok := m.workers[key]; ok {
				worker.stop()
			}
		}
```
Called from `pkg/kubelet/kubelet.go:2388` in `SyncTerminatingPod`, after `SetPodStatus` and immediately before `killPod` (which runs `preStop` and sends SIGTERM): `kl.probeManager.StopLivenessAndStartup(pod)`.

Readiness keeps probing, in the source's own words, `prober_manager.go:190-194`:
```go
	// Detach the workers' context from the caller's: the pod worker cancels the
	// sync context when the pod begins terminating, but probe workers must keep
	// probing until the container stops so a failing readiness probe can mark
	// the pod NotReady during graceful termination. Workers are stopped
	// explicitly via their stop channel (RemovePod/CleanupPods).
```
The loop `worker.go` exits only when the pod phase is Failed or Succeeded (`worker.go:228-233`).

Endpoints marked terminating (Spec, kubernetes.io docs): `content/en/docs/concepts/services-networking/endpoint-slices.md`: "The `terminating` condition indicates that the endpoint is terminating. For endpoints backed by a Pod, this condition is set when the Pod is first deleted (that is, when it receives a deletion timestamp, but most likely before the Pod's containers exit)." `pod-lifecycle.md` (termination flow): the kubelet runs the `preStop` hook, then the container runtime sends TERM, and "At the same time as the kubelet is starting graceful shutdown of the Pod, the control plane evaluates whether to remove that shutting-down Pod from EndpointSlice objects"; "Terminating endpoints always have their `ready` status as `false` ... so load balancers will not use it for regular traffic", and the `serving` condition "maps to the Pod's `Ready` condition" so it keeps following readiness.

Two corrections to the response's wording:

1. The endpoint is marked terminating when the pod gets its deletion timestamp, not "when SIGTERM is sent". SIGTERM follows `preStop`. So the app's lag after SIGTERM starts later than the endpoint change; `lag` as "the `preStop` sleep's equivalent" in Q6 is accurate only if no `preStop` exists (with one, the endpoint change already had that long to propagate). The effect is that `lag = 0` is safer in Kubernetes than the response suggests when a `preStop` is present, and the recommendation to document "a few seconds" holds for deployments without one.
2. The readiness flip at `Stopping` is not invisible to Kubernetes: the `serving` endpoint condition follows pod Ready, and readiness probes keep running, so a client that reads `serving` (the response's "readers of readiness") sees the flip. `ready` is already false.

Kubernetes treats HTTP status 200 to 399 as probe success (docs: "Any code greater than or equal to 200 and less than 400 indicates success"; matches section 3 of the first response). The gRPC probe sends `Check` with `service` defaulting to the empty string (`k8s.io/api core/v1 types.go`, `GRPCAction.Service`, `+default=""`), so the overall status `""` is what the kubelet reads (confirms Q2(b)).

Verdict: **holds** for liveness/startup stopping and readiness continuing; the "marked terminating at SIGTERM" timing is **partly** (deletion timestamp, before `preStop`).

---

## 7. This repo: what carries a trace context today, and the insertion points

Search: `grep -rn -i -E 'traceparent|tracestate|TraceContext|trace_context|w3c' crates` finds only `crates/ulo-rpc/src/transport.rs:212` (a doc comment: "`traceparent` (W3C Trace Context) is an ordinary header ... nothing in ulo reserves, writes or refuses it") and the F400 test in `crates/ulo-rpc/src/client.rs:1587-1597`. There is no `TraceContext` type, no `TraceMode`, no `ExecOptions::trace`, and no `Execution::trace` anywhere. Read.

### Where `TraceContext` lives

`crates/ulo/src/execution/mod.rs` (the core; runtime-free):

- `Execution` `:41`, `ExecutionRef` `:49`, `ExecOptions` `:55` (one field, `deadline` `:56`), `ExecShared` fields `:~105-135` (`deadline` at `:122`; `extensions` and `inputs` beside it). Add `trace: Option<TraceContext>` to `ExecOptions` (`:55`) and `ExecShared` (next to `:122`), copied at `crates/ulo/src/app/shared.rs:172` where `deadline: opts.deadline` is copied from `opts`. Add `trace()` on `Execution` (`:205` area) and `ExecutionRef` (`:287` area), mirroring `deadline()`. The type itself fits in a new `crates/ulo/src/execution/trace.rs` (std only; `p5-w3c/src/lib.rs` is the shape).
- `AppBuilder::trace(TraceMode)`: `crates/ulo/src/app/mod.rs` (where the builder's other settings sit); the mode reaches transports through `AppShared`/`AppHandle` the same way `timer` does. Not located further; Unverified.
- Seeding order matters: `ExecOptions` is consumed by `Execution::open` (`:145`), so each transport must extract the incoming context before it opens the execution, the way RPC already computes `deadline` into `ExecOptions` (`crates/ulo-rpc/src/dispatch.rs:192-224`). `Execution::seed` (`:159`) exists but takes arbitrary inputs, not a typed trace slot, so it is the wrong vehicle: the core needs `Execution::trace()` to be a typed read.

### How `.within(&exec)` forwards it

- RPC: `Spec` `crates/ulo-rpc/src/client.rs:438` holds `within: Option<ExecutionRef>` (`:444`). All three shapes pass through one function that already stamps `deadline-ms`: `written(headers, ends, stamps, now)` at `:593-602`, called at `:556` (request and stream: `open`) and `:1020` (event: `Emit::into_future`, after `limit(Whole::Publish)` at `:1010`). Inject `traceparent` and `tracestate` there, from `within.trace()`. `Frame::Req`, `Frame::Open` and `Frame::Evt` all carry `headers: CallHeaders` (`crates/ulo-rpc/src/frame.rs:35,36,41`), so the premise "every frame kind carries `h`, events included" holds (Read). Reserved-header refusal: extend `Spec::reserved` (`:536-543`, called at `:951`, `:1009`, `:1115`) to `traceparent` while tracing is on, with reason `reserved_header` as for `deadline-ms`. `tracestate` merge also happens in `written`.
- gRPC: `ulo_grpc::outgoing(exec, request)` `crates/ulo-grpc/src/client.rs:120-127` returns the request unchanged when `exec.deadline()` is `None` (line 121), so the trace injection must be placed before that early return, writing `traceparent` into `request.metadata_mut()`. `timed` (`:134-137`) shows the pattern for mutating the request.
- HTTP outgoing: no HTTP client exists in the framework. Capability 26 says "every transport the framework serves and calls"; the framework calls over RPC and gRPC only, so HTTP is server-side extract only. Read (no `ulo-http` client module in `crates/ulo-http/src`).

### Extract points (before `Execution::open`, with the file and line)

| Transport | Where headers are available | Open | Span creation | Notes |
| --- | --- | --- | --- | --- |
| HTTP | `req.head.headers` at `crates/ulo-http/src/service.rs:160` (`respond`) | `service.rs:160` `Execution::open(&self.app.root(), ExecOptions::new())` | `service.rs:176` `span::call(...)`, `.instrument` at `:181` | Seeds `RequestHead` at `:163`. |
| gRPC | `req.into_parts()` at `crates/ulo-grpc/src/dispatch.rs:160-162`, or `head.headers` (HTTP/2 headers are the metadata) | `dispatch.rs:154-158` | `dispatch.rs:169` `call_span(head.uri.path())`, defined `:508` | `opts` built from `grpc-timeout` at `:154-157`; `headers` must be read before `req.into_parts()` is consumed at `:160`, or from `req.headers()` earlier. |
| RPC | `incoming.headers` (`CallHeaders`, `crates/ulo-rpc/src/transport.rs:215`) at `crates/ulo-rpc/src/dispatch.rs:192` | `dispatch.rs:224` (and `:278` for an unhandled pattern) | `dispatch.rs:661-662` `call_span(shared, pattern, handler)` | `opts` built at `:220-223` from `deadline-ms`; events run through the same path (`Arrived::Evt`), so the event's span gets the emitter as parent with no extra code. |
| WebSocket connect | `UpgradeHead` (handshake `Parts`), `crates/ulo-ws/src/connection.rs:582` `handshake(accept, head: Parts, ..)` | `connection.rs:798` `Execution::open(connect.module(), ExecOptions::new())` | `connection.rs:838` `span::call(.., "ws.connect", ..)` | The connection's own span becomes the parent of each message's execution; keep its `TraceContext` on the connection (`Connection::seed`, `connection.rs:175-179`, is where per-connection inputs are seeded into every execution). |
| WebSocket message | the connection's stored context | `connection.rs:1391` `Execution::open(&module, ExecOptions::new())` | `connection.rs:1401` `span::call(.., &name, ..)` | Also `connection.rs:143` (`open_execution`, hand-written gateways) and `:1527` (disconnect cleanup). |

Wherever a span is created, the new hook (section 2) runs between the `span::call(..)` line and its `.instrument(call_span...)`. Add `otel.kind = Empty` and `otel.status_code = Empty` to `span::call` (`crates/ulo-transport/src/span.rs:38-54`) so SERVER kind and the 5xx rule can be recorded.

### Inject points

- RPC client: `crates/ulo-rpc/src/client.rs:593` `written` (all shapes, including events).
- gRPC client: `crates/ulo-grpc/src/client.rs:120` `outgoing`.
- Standalone `execute`: Q5's `ExecOptions::trace(TraceContext)` goes beside `ExecOptions::deadline` at `crates/ulo/src/execution/mod.rs:66`.

---

## Summary table

| Premise | Verdict | Recommended choice | Confidence |
| --- | --- | --- | --- |
| tonic-health `set_service_status` notifies `Watch` immediately | Holds (104 us on loopback) | Use `HealthReporter` as is | High (Probed) |
| `Watch` behaves per `grpc.health.v1` | Partly: initial status, updates and `Check("")` hold; `Watch` of an unregistered service ends `NOT_FOUND` instead of streaming `SERVICE_UNKNOWN`; same-value writes send duplicate messages | Document the deviation (Kubernetes reads `Check`); the 5 s re-evaluation writes only on change | High (Probed, Read) |
| `""` written by one component, named services by another, no overwrite | Holds at the data level (independent per-key channels); no ownership check, last writer per key wins | `GrpcHealth::delegate_overall()` as in Q2(c) | High (Probed, Read) |
| tracing-opentelemetry can parent a span on an extracted `traceparent` | Holds via `OpenTelemetrySpanExt::set_parent` after creation and before first enter; a field-reading layer is not possible (internals are `pub(crate)`) | Core-owned `TraceSource` hook called between `span::call` and `.instrument`; `ulo-otel` implements it with `set_parent` then `span.context()`. Add `otel.kind` and `otel.status_code` fields to `span::call` | High (Probed, Read) |
| Versions | tracing-opentelemetry 0.34.0 with opentelemetry and opentelemetry_sdk 0.33.1 and opentelemetry-otlp 0.33.1 | Pin that set | High (Probed) |
| `opentelemetry_sdk` without `rt-*` pulls no tokio | Holds; features `opentelemetry_sdk = { default-features = false, features = ["trace"] }`, `opentelemetry = { default-features = false }`; avoid `testing` | Use exactly that set; CI asserts `cargo tree -i tokio` exits non-zero | High (Probed) |
| `BatchSpanProcessor` exports from its own thread | Holds (thread `OpenTelemetry.Traces.BatchProcessor`) | As designed | High (Probed) |
| `shutdown` blocks and can deadlock on a current-thread runtime | Partly: blocks; stalls up to its timeout (default 5 s) only when the exporter needs the runtime; returns `Ok` immediately with a runtime-independent exporter | Keep the off-thread flush with a oneshot under the hook's bound (Q9) | High (Probed) |
| `opentelemetry-otlp` default features and tokio | Defaults are `http-proto` and `reqwest-blocking-client` (plus trace, metrics, logs); tokio is in the tree through reqwest | Use `default-features = false` with `http-proto` and `trace` | High (Probed, Read) |
| OTLP HTTP export works without a caller runtime | Holds (sync main; also from a current-thread runtime's thread); a request arrived with the span and protobuf body | As designed | High (Probed) |
| `ulo-otel-otlp` must be tokio-bound | Does not hold: with a 20-line `HttpClient` over ureq the whole tree has no tokio and exports | Ship the ureq-backed client as default, `reqwest-blocking` as an opt-in feature; record both in `runtime-levels.txt` | Medium (works; client TLS and proxy behavior not evaluated) |
| W3C: invalid `traceparent` restarts, `tracestate` dropped | Holds, plus harness-only rules (duplicate header restarts, OWS trimmed, bad `tracestate` never affects `traceparent`) | Implement the rule list in section 5; reference parser passes 16 tests and fails on a planted break | High (Spec, Probed) |
| W3C: higher-version forward compatibility | Holds (parse by `00` offsets, end-or-dash after flags, output version `00`, unknown flags zeroed) | As in section 5 | High (Spec, Probed) |
| W3C: `tracestate` limits and mutation | 32 members, key 256 or `tenant@system` 241/14, value 256, propagate at least 512 characters, truncate whole entries (over 128 first, then from the right); mutation rules are section 3.5, not 3.3 | As in section 5; keep `flags` as bits and decide Level 1 vs Level 2 (`random` flag, 0x02) | High (Spec, Probed); Level choice open |
| Kubernetes: liveness/startup stop at termination, readiness continues | Holds (`prober_manager.go:244-258` and `:190-194`, `kubelet.go:2388`) | Keep section 2.2 as written | High (Read) |
| Endpoint marked terminating when SIGTERM is sent | Partly: set at the deletion timestamp, before `preStop` and SIGTERM; `serving` condition follows pod Ready | Reword Q6's `lag` note: it is the `preStop` sleep's equivalent only when no `preStop` exists; keep default zero | High (Spec, Read) |
| Core can carry a trace context today | Does not hold: no type, no mode, no slot; `traceparent` is an ordinary header (`ulo-rpc/src/transport.rs:212`) | Add `TraceContext` in `crates/ulo/src/execution/`, `ExecOptions::trace` at `:55`, `ExecShared` field beside `:122`, copy at `app/shared.rs:172` | High (Read) |
| Every RPC frame kind carries `h`, events included | Holds (`Frame::Req`, `Open`, `Evt` all have `headers`) | Inject once, in `written` (`ulo-rpc/src/client.rs:593`) | High (Read) |
