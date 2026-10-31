# Switch inventory: what master's `examples/` and `integration-tests/` covered that the new framework does not yet

Read-only gap check on `experiment/di-redesign` at `2775d54c`, against master's two excluded trees
(F335). The new framework's surface, as the designer's catalogue defines it, decides which examples
and wiring tests exist. This file does not propose ports. It lists (1) what master exercised, the new
framework still has, and nothing now covers; (2) what master exercised that no longer exists; (3)
what is already pinned elsewhere; (4) the per-file evidence; (5) the capability areas the designer
writes the catalogue from.

Method: every master file read at its header and test list; the new framework searched for the API
each uses (`crates/ulo/src`, `ulo-transport`, `ulo-http`, `ulo-ws`, `ulo-rpc`, `ulo-grpc`,
`ulo-config`, `ulo-health*`, `ulo-graphql*`, `ulo-otel`, `ulo-db*`); coverage searched in every
`crates/*/tests`, `crates/*-conformance/src/cases`, `ulo-codegen-tests`, `ulo-runtime-tokio/tests`,
the `ulo new` templates, `scripts/template_matrix.py` and `.github/workflows/ci.yml`. Nothing was
compiled or run. `unverified` marks a claim inferred from names or doc comments, not from reading a
test body. `crates/*/examples/` is empty in every crate, and `ci.yml` never builds `examples/` or
`integration-tests/`.

Templates: `ulo new` writes one greeting handler per transport (`http`, `ws`, `grpc`, `rpc-tcp`,
`rpc-udp`, `rpc-nats`, `rpc-redis`, `rpc-rabbitmq`, `rpc-mqtt`, `rpc-kafka`) with `Greeter`
(`#[injectable]`, `Dep<Greeter>`), a layered `Config` (`File::toml` + `Env::process`), a
`TestApp::of(AppModule).wire()` test, `serve(shutdown_signal())`, gRPC health, ten database
variants, and `ulo generate` output; CI builds, tests and clippies each.

## 1. Gap list

Each entry: a concept master exercised, the new crate/API that has it, and the search that found no
cover. "Searched" is a grep over `crates/*/tests`, conformance `src/cases`, the templates and
`ulo-codegen-tests`. Each is a candidate for the designer, not a port. Ordered by how much a user
relies on it and how little pins it.

| # | Concept | New location | Searched, found nothing | Master files that exercised it |
| --- | --- | --- | --- | --- |
| G1 | Scopes: singleton, per-execution, transient; `Auto` inference; a controller depending on per-execution data is built per call; singleton depending on per-execution refused | `ulo/src/scope.rs`, `graph/scopes.rs` | `PerExecution`, `Transient`, `Scope` in all `tests/` and cases: only `ulo-runtime-tokio/tests/app.rs` touches `TestApp`, and not scopes. `crates/ulo` has no tests at all | `provider_scope`, `concurrent_scope`, `execution_scope_sharing`, `scope_bubbling`, `di_core`, `di_validation`, `ws_execution_scope`, examples `derive_injectable` |
| G2 | Wiring errors reported in one pass: missing dependency, duplicate binding naming both modules, scope violation, import and dependency cycles with the full path | `ulo/src/error/wiring.rs`, `graph/cycles.rs`, `graph/register.rs` | `WiringError`, `cycle` in tests: none. The template test only wires the happy path | `circular_dependency`, `import_cycle`, `deferred_instantiation` |
| G3 | Modules: export-only visibility, `global()`, `for_root` config identity (same config deduped, different config two modules), keyed instances, `reexport`, `ModuleRef`, lazy `App::load` | `ulo/src/module/*`, `graph/visibility.rs`, `app/handle.rs` | `reexport`, `global()`, `module_keyed`, `Keyed`, `load(` in tests: only DB `keyed::two_instances` (one use of keyed) | `modules`, `global_modules`, `module_ref`, `module_lookup`, `dynamic_module_controllers`, `graphql_module_identity` |
| G4 | Bindings: value, factory (async, fallible), alias, qualifier, `contribute`/`Many<T>`, `Option<Dep<T>>`, third-party type, single-vs-collection refusal | `ulo/src/binding/*`, `module/def.rs`, `dependency/*` | `Many<`, `contribute`, `alias`, `try_with` in tests: none | `di_providers`, `multi_providers`, `provide_macro`, `token_format`, `new_ctor_injection`, examples `provider_patterns`, `derive_injectable` |
| G5 | `TestApp` overrides: `override_value`, `.in_module`, `replace_module`, stale override is a wiring error | `ulo/src/testing.rs` | `override_value` appears once, in `ulo-runtime-tokio/tests/app.rs` (runtime binding, not override semantics, unverified) | none in master; new capability 48 |
| G6 | Lifecycle hooks: init then bootstrap order, dependency order, module hooks, failing hook fails startup, shutdown order and signal argument, hook on non-singleton refused | `ulo/src/hooks.rs`, `lifecycle/*`, `module/def.rs` | `OnModuleInit` in tests: none; `on_init` only in two runtime tests (task spawning). `ulo-runtime-tokio/tests/app.rs` pins close claims, not order (unverified) | `lifecycle_hooks`, `derive_lifecycle_hooks`, example `lifecycle_hooks` |
| G7 | Enhancer pipeline order and laziness: global > controller > method, guard then interceptor, each guard built only after the one before admits, error handlers reverse order, type/value/closure spellings | `ulo/src/transport/pipeline.rs`, `enhancer.rs`, ADR-0055 | only gRPC (`ulo-codegen-tests/tests/enhancers.rs`) pins impl/method tiers; nothing pins the order across tiers or the build-laziness | `enhancers`, `enhancer_declaration_order`, `guards_build_in_order`, `error_handler_precedence`, `global_enhancers`, `inline_enhancers`, `enhancers_di`, `scoped_enhancers` |
| G8 | Execution data: `Ext<T>`, `Extensions`, guard writes and handler or deep service reads, standalone `App::execute`, per-execution cache | `ulo/src/dependency/ext.rs`, `execution/*` | `Ext<`, `Extensions::insert`, `execute(` in tests: none | `extension_di`, `extension_bus`, `extension_bag_reaches_providers`, `guard_mut_context`, `resolve_by_hand`, `ws_connect_execution`, examples `execution_scoped_context`, `multi_protocol_context` |
| G9 | HTTP routing details: trailing slash insignificant, `{name}` parameters, duplicate route refused, `{*rest}` | `ulo-http/src/router/pattern.rs` | `trailing`/`slash` in tests: none; conformance routing app has no path-parameter route | `trailing_slash_conformance`, `param_syntax_conformance` |
| G10 | CORS beyond one preflight (credentials, allow-list, max-age, non-preflight decoration); tower layers in the pre-dispatch stage; per-module middleware by route with exclusions; URI rewrite changing the route | `ulo-http/src/cors.rs`, `tower_bridge.rs`, `middleware.rs`, `pre_dispatch.rs` | `Cors` appears in `ulo-http-conformance/src/app.rs` with one `preflight` scenario; `tower_bridge`, `Middleware` impls: only `ulo-codegen-tests/tests/enhancers.rs` (two middlewares, on a gRPC app) | `cors`, `tower_compat`, `global_chain_conformance` (rewrite case), `middleware_error`, example `middleware_examples` |
| G11 | SSE wire: headers, `data`/`id`/`event`/`retry`/comment framing, multi-line data, `keep_alive`, `end_event`, `LastEventId` | `ulo-http/src/sse.rs`, `extract/head.rs:51` | `Last-Event-ID`, `keep_alive` in HTTP tests: none; conformance has only `sse::error_event` | `sse`, example `sse` |
| G12 | HTTP extractors: `Path`, `Query`, `Json`, `Form`, `Multipart`, `BodyStream`, `Host`, `Header`; `Valid<P>` on HTTP/RPC/WS | `ulo-http/src/extract/*`, `ulo-transport/src/validate.rs` | `Multipart` in tests: none; `Valid` is only in the conformance app (`valid` route, failure shape); RPC/WS `Valid` untested | `extractors`, `body_and_multipart_extractors`, `streaming`, `validated_transports`, examples `file_upload`, `validation_complete_guide`, `custom_extractors` |
| G13 | Handler forms and returns: sync/async, no `&self`, return alias of `Result` reaching the chain, `IntoReply` | `ulo-transport/src/reply.rs`, `ulo-http-macros` | HTTP conformance handlers are all sync and unaliased; `handler_forms` equivalent absent | `async_controllers`, `static_methods`, `handler_return_is_a_type`, `request_extractor` |
| G14 | Error model: `Classify` derive and kind-to-wire mapping on HTTP, chain claim vs fall-through, reshape | `ulo-transport/src/error.rs`, `ulo-transport-macros` | `ErrorKind::` in rpc conformance only; HTTP status-per-kind table not asserted (unverified); no `Classify` derive test | `error_derive`, `error_handler`, `error_handler_answers_a_handlers_type`, example `error_handling` |
| G15 | Declared metadata `#[meta]` on HTTP and RPC (WS and gRPC covered) | `ulo/src/transport/metadata.rs` | `ulo-ws-hyper/tests/messages.rs` and gRPC `enhancers.rs` only | `metadata_overlay`, example `route_metadata` |
| G16 | Configuration: layered sources (defaults, file, `.env`, env, values), section by path, aggregated errors naming path and source, secrets | `ulo-config/src/*` | `ulo-config/tests` hold only `secret_leak`, `yaml_alias_bomb`; unit tests only in `source.rs`. Template test uses `Values` | `config_injection`, examples `config_module`, `config_validation` |
| G17 | Health surfaces: HTTP probe paths, status codes, body, detail off by default; readiness false at drain before first refusal; `grpc.health.v1` `Watch` | `ulo-health-http`, `ulo-health-grpc`, `ulo-health` | readiness through `HealthModule` is covered by `ulo-db-conformance/src/cases/health.rs`; gRPC health serving and drain by `ulo-codegen-tests/tests/builtins.rs`; HTTP probe responses: none | example `health_checks` |
| G18 | GraphQL: mount, playground as HTML, module identity, subscriptions over graphql-transport-ws for both engines | `ulo-graphql`, `-http`, `-ws`, `-async-graphql`, `-juniper` | only `ulo-graphql-ws/tests/runtime.rs` (task spawning) | `graphql_*` (5 files) |
| G19 | gRPC: TLS serving, `grpc-status-details-bin`, in-flight bound, service not injectable | `ulo-grpc/src/server.rs`, `status.rs`, `ulo-transport/src/admission.rs` | `tls` absent from `ulo-codegen-tests`; `Details` untested; no gRPC admission test | `grpc_tls`, `grpc_metadata_and_status_details`, `rpc_grpc_macros` backpressure, `grpc_service_injection` |
| G20 | RPC: a controller cannot be injected; `RpcClientModule` injection; declared metadata; `Valid` payload | `ulo-rpc/src/client.rs`, `module/def.rs` | `ulo-rpc/tests` cover runtime/drain/deadline only | `rpc_controller_injection`, `validated_transports`, example `rpc_nats_client` |
| G21 | One controller serving several transports; one app serving HTTP, WS, RPC and gRPC together; one `close` draining all | `Controller`/`Mount` in `ulo/src/transport/controller.rs` | no test builds more than one transport kind except `ulo-ws/tests/handoff.rs` (HTTP+WS) and `enhancers.rs` (gRPC+middleware) | `http_e2e`, `graceful_shutdown`, `an_application_is_send`, `bind_refusals`, `bind_phase_order`, examples `graceful_shutdown`, `gateway_http_bridge` |
| G22 | Startup failure shape: `StartupError::{Wiring,Connect,Configure,Bind}`, a transport error surviving as `source`, redaction | `ulo/src/error/mod.rs` | `redacted` in runtime conformance covers task panics; no test of the variants (unverified) | `adapter_error_contract`, `bind_refusals`, `provider_startup_check` |
| G23 | Supervision: a serve loop that dies closes the app | `ulo/src/app/mod.rs` (`App<Bound>::serve`) | `App<Bound>::serve` in `crates/ulo/src/app/mod.rs` (~498-559) supervises every server's serve future; pinned by `crates/ulo-runtime-tokio/tests/app.rs` `a_transport_whose_serve_fails_closes_the_app` (:1187), `a_transport_whose_serve_returns_while_the_app_runs_closes_the_app` (:1192), `two_serves_ending_on_one_poll_record_the_first_bound_once` (:1252) and by `crates/ulo/tests/serve_discard.rs` | `serve_loop_death` |
| G24 | Tracing: span per call, W3C propagation across transports, OTLP export | `ulo-transport` span, `ulo-otel` | `traceparent` in all tests: none; `ulo-otel` has no tests | example `rpc_tracing` (spans only) |
| G25 | WS per-message execution scope; connect-guard bag reaching `on_connect` | `ulo-ws`, `ulo-ws-hyper` | `PerExecution` absent from `ulo-ws-conformance/src/app.rs` | `ws_execution_scope`, `ws_connect_execution` |
| G26 | Examples with no cover anywhere: custom extractor, SSE, rooms, client, streaming, lifecycle, middleware, file upload, validation, socket activation (adoption itself is pinned by `ulo-http-hyper/tests/listeners.rs`) | as above | `examples/`: none in the new tree; templates show only the greeting | 42 master examples, appendix A1 |

## 2. Dropped list (concepts that no longer exist)

For the docs site and beta release notes. Evidence: crate or ADR or design capability.

| Master concept | Replaced or removed by | Evidence |
| --- | --- | --- |
| Default logger installed by `create` (`try_init`, `RUST_LOG`, `logger` feature) | the application installs its own subscriber; OpenTelemetry via `ulo-otel` | no `try_init` in `crates/ulo`; template `common/main.rs` calls `tracing_subscriber::fmt::init()`; `ulo-runtime-tokio/tests/own_subscriber.rs` |
| String DI tokens; `provider_value!`, `provider_factory!`, `provider_alias!`, `provider_token!`, `provide!`; `#[inject("TOKEN")]`; `Token<T>` | compiler-checked keys; `m.value`, `m.with`, `m.alias`, `qualified`, `contribute` | core capability 9; `ulo-macros` exports (`construct, error_handlers, guards, injectable, interceptors, meta, module, routes`); `ulo/src/key.rs` |
| Role by trait detection and marker-free enhancers; string-token enhancers; `use_global_*` on `UloFactory` | typed role keys `AnyGuard<T>`, `AnyInterceptor<T>`, `AnyErrorHandler<T>` registered with `m.enhancer::<R>()`; global middleware in `PreDispatch` | core capabilities 25, 28; `ulo/src/transport/mod.rs:134`; ADR-0055, ADR-0056 |
| `UloFactory`, `UloApplication`, `create`, `use_http_adapter`, `BindTarget`, `use_websocket_listener`, `BoundAdapters`, `create_application_context` | `App::builder(root).runtime(..).wire()?.connect().await?.bind(..).listen().await?.serve(..)`; `ulo-net` listeners and `activation.rs` | `ulo/src/app/*`; template `common/main.rs` |
| Adapter SPI (`HttpAdapter`, `WsAdapter`, `RpcAdapter`, `GrpcAdapter`), `add_service`, `ulo-ws-tungstenite`, per-adapter WS | `Server` + `Mount`; `Embed` for hosts; `ulo-ws-hyper` and the `ulo-ws` hand-off | `ulo/src/transport/server.rs`; transports DESIGN §3.8; `ulo-http-conformance` |
| `#[controller]`, `#[patterns]`, `#[subscriptions]`, `#[grpc_methods]`, `#[websocket_gateway]`, `#[message_pattern]`, `#[event_pattern]`, `#[subscribe_message]`, `#[sse]` | `#[routes]` plus one attribute per transport: `ulo_http::get` etc., `ulo_ws::gateway`/`message`, `ulo_rpc::message`/`event`, `ulo_grpc::method`; SSE is `Sse::new(stream)` | `ulo-*-macros` `lib.rs`; templates |
| `#[use_guards]`, `#[use_interceptors]`, `#[use_error_handlers]`, `#[catch]`, `#[set_metadata]`, `ErrorHandler` returning `Option` | `#[ulo::guards]`, `interceptors`, `error_handlers`, `#[meta]`; handlers return `Result<Reply, BoxError>` | `ulo-macros/src/lib.rs`; `ulo-codegen-tests/tests/replies.rs` |
| `#[body]`/`#[param]`/`#[query]` markers; `Body<T>` content sniffing; `take_body`, `CONSUMES`, `BodyAlreadyRead`; extractor chosen by written type name | typed `Json`, `Form`, `Path`, `Query`, `Multipart`, `BodyStream`; `Param<T, M>`; extractor trait `FromCall<T>` | `ulo-http/src/extract/*`; `ulo-transport/src/extract.rs`; grep of `take_body` empty |
| `Extension<T>`, `Validated<E>`, `HttpContext`/`WsContext`/`RpcContext`/`GrpcContext`, `ProviderContext`, `Request` provider | `Ext<T>`, `Valid<P>`, `HttpCx`/`WsCx`/`RpcCx`/`GrpcCx`, `Resolver`/`App::execute`; no framework-registered request provider found (unverified) | `ulo/src/dependency/ext.rs`; `ulo-transport/src/validate.rs`; conformance apps |
| `#[on_module_init]` and the other hook attributes | traits `OnModuleInit` etc. and `m.on_init(..)` | `ulo/src/hooks.rs`, `module/def.rs` |
| `#[derive(Config)]`, `#[env]`, `ConfigService<T>`, panic on load failure | layered `Config::builder()` sources, section by path, all errors reported at `wire` | integrations capabilities 1-4; `ulo-config/src`; template `app.rs` |
| `BroadcastService`, `BroadcastModule`, `WsClient` handler param | `Rooms`, `RoomsIn`, `Broadcast`, `Hub`; `Session` | `ulo-ws/src/rooms.rs`, `session.rs` |
| `CorsMiddleware`, `TowerLayer`, `apply_tower`, feature `tower-compat` | `Cors` entry in `PreDispatch`; `ulo_http::tower_bridge::Service` | `ulo-http/src/cors.rs`, `tower_bridge.rs`; `crates/ulo/TOWER_COMPAT.md` is stale |
| `HealthCheckService`, `HttpHealthIndicator`, `MemoryHealthIndicator`, `DiskHealthIndicator` | `Check`, `Health`, `Probe`, `HealthModule`, `Readiness`; probes in `ulo-health-http`/`-grpc` | `ulo-health/src/lib.rs`; memory/disk indicators not found (unverified) |
| gRPC manual (no-proto) trait form, `shapes = path`, `tonic::Request<T>` param, `GrpcRequest`, tonic client registered by hand | method shape from `#[ulo_grpc::method(pb::svc::Method)]` and `Message<T>`; `GrpcClientModule::for_root(GrpcEndpoint)`; compile-fail checks | `ulo-grpc/src/client.rs`; `ulo-codegen-tests/tests/ui/grpc_*` |
| Prisma module | removed: upstream archived 2025-03-12 | commit `8726c104`; integrations capabilities 11 |
| `anyhow`-era result aliases (`InitResult`, `AdapterResult`, `SetupResult`), `StartupError::{HookFailed,Adapter,Setup}` | `BoxError`; `StartupError::{Wiring,Connect,Configure,Bind}` | `ulo/src/error/mod.rs:31` |
| `coverage_ledger.rs` as a listing derived from `integration-tests` | no owner; its `suite` column no longer describes where proof lives | section 3 note |

## 3. Redundancy: master files whose behaviour a named suite now pins

Brief; the per-file evidence is in the appendix.

| Master | Pinned now by |
| --- | --- |
| HTTP routing, 404/405/OPTIONS/HEAD, extraction failure shape, error reshape/recovery, SSE error event, upgrade, disconnect, drain, host values, preflight (`global_chain_conformance`, `extraction_failure_shape`, `panic_recovery`, `cancellation`, `sse_adapter_conformance`, parts of `graceful_shutdown`) | `ulo-http-conformance` scenarios `hit`, `not_found`, `method_not_allowed`, `options`, `head`, `routing`, `failures`, `reshaped`, `recovered`, `error_event`, `echo`, `preflight`, `unavailable`, `mid_stream`, `http1`/`http2`; stamped by every `crates/ulo-http-*/tests/conformance.rs` |
| Listener adoption and hand-off (`bind_target_*`, `ws_listener_adoption`, `rpc_grpc_listener_adoption`) | `ulo-http-hyper/tests/listeners.rs`, `ulo-listener-{tokio,smol}/tests/listener.rs`, `ulo-net/src/activation.rs` |
| WebSocket wire, close codes, refusals, hooks, sessions, streams, rooms (`ws_*`, `websocket_e2e`, `websocket_broadcast`) | `ulo-ws-conformance` `handshake`, `close_codes`, `hooks`, `keep_alive`, `limits`, `messages`, `stream_end`; `ulo-ws-hyper/tests/{attributes,handshake,hooks,limits,messages,stream_end}.rs`; `ulo-ws/tests/{handoff,broadcast}.rs` |
| RPC tcp/udp wire, panics, drain, backpressure, streams, cancel, metadata, binary (`rpc_tcp*`, `rpc_udp*`, `rpc_binary_payload_is_refused`) | `ulo-rpc-conformance` `unary`, `streams`, `errors`, `misses`, `events`, `headers`, `payloads`, `admission`, `cancel`, `drain`, `deadlines`, `recovery`; `ulo-rpc-{tcp,udp}/tests` |
| gRPC call shapes, enhancers, panics, deadlines, errors, reply metadata, reflection, health, client, stream tail (`rpc_grpc*`, `grpc_*`) | `ulo-codegen-tests/tests/{grpc,enhancers,replies,deadlines,clients,builtins,stream_end}.rs` and `ui/` compile-fail |
| Guard/panic/unrouted as events (`guard_rejection_is_an_event`, `guard_panic_is_an_event`, `pipeline_panic_is_an_event`, `unrouted_is_an_event`, `ws_panic_recovery`) | RPC `errors::{guard_refusal,panic}`, `misses::*`; WS `close_codes::*`, `messages::*`; gRPC `replies.rs`; HTTP `errors::recovered` |
| DB module startup checks (`provider_startup_check`) | `ulo-db-conformance` `connect::*`, `health`, `lazy`, `keyed`, `migrate` via each `ulo-db-*/tests/conformance.rs` |
| Examples `hello_world`, `websocket_chat`, `rpc_controller`, `rpc_udp`, `rpc_nats` | the `ulo new` templates for `http`, `ws`, `rpc-*`, built and tested by CI `template-matrix` |
| Four adapter proofs of concept | per-crate `conformance.rs`/`ws_conformance.rs`: the HTTP conformance suite runs against every adapter, and example 21 shows embedding |

`coverage_ledger.rs` (the "every crate is proved somewhere" assertion) has no owner now: `ci.yml`
asserts nothing per crate, its `suite` column means "registered in integration-tests", and it names
the removed `ulo-db-prisma`. Whether the question is still worth asking is for the designer.

## 4. Totals

Appendix rows (mixed files split): GAP 91, COVERED 68, DROPPED 15,
UNKNOWN 6. GAP means a concept that survives and has no cover; consecutive rows often
repeat one concept, so section 1 holds 26 distinct gaps. Master inventory: 42 examples, 132 test
files under `integration-tests/tests/integration/` (548 `#[test]`/`#[tokio::test]` attributes).

## 5. Inputs for the designer: capability areas of the new framework

From `CAPABILITIES.md` (core), `integrations/CAPABILITIES.md`, `transports/CAPABILITIES.md` and the
templates. Numbers are the capability numbers there.

Core DI (48 capabilities)
- Bindings 1-10: service, value, async factory, trait binding, qualifiers, alias, contributions, third-party type, typed keys, duplicate refusal
- Injection sites 11-17: one rule for fields, constructors, factories; trait, collection, per-execution view; optional; owned state; fallible constructors; type not spelling; shared instances
- Scopes and executions 18-23: singleton, per-execution, transient; refusal and auto-elevation; execution cache, extension bag, cancellation, deadline
- Enhancers and roles 24-28: guards, interceptors, error handlers, middleware as bindings; role from the trait per transport; by type, value, closure; lazy build order; role keys
- Modules 29-38: imports/exports, global, `for_root`, keyed instances, runtime-built, re-export, identity, per-module middleware, `ModuleRef`, lazy load
- Lifecycle 39-43: five hooks, dependency order, singleton-only, startup checks, phases
- Errors and diagnostics 44-47: one-pass wiring errors, no panics, compile errors, typed lookups
- Testing 48: `TestApp` overrides

Integrations (26)
- Configuration 1-5; Health 6-10; Databases and stores 11-21 (sqlx postgres/mysql/sqlite, seaorm, diesel, mongodb, redis; named instances; migrations; runtime refusal); Command line 22-25 (`ulo new`, `ulo generate`, `ulo dev`); Observability 26 (W3C trace context, OTLP)

Transports (39)
- Common 1-14: handlers and extractors, validation, typed answers, one error model, metadata, cancellation, graceful shutdown, binding and pre-bound listeners, TLS, load shedding, transport keys, controller-level enhancers, tracing span
- HTTP 15-21: routing, backends and embedding (hyper default; axum, salvo, poem, actix, rocket), extractors, responses and SSE, pre-routing and module middleware, CORS and tower, WebSocket upgrade
- WebSocket 22-27: gateways and events, connect guards and close codes, sessions, hooks, replies and envelope, rooms and Redis broadcast
- RPC 28-33: request/event patterns, four call shapes, seven transports (tcp, udp, nats, redis, rabbitmq, mqtt, kafka), client, miss reporting, binary payloads
- gRPC 34-36: build step, call shapes, deadlines and status details, reflection, health, TLS, clients
- GraphQL 37: two engines, subscriptions, playground
- Runtime and tooling 38-39: `ulo-runtime-tokio`/`-smol`, signals, `ulo dev --listen`

Template matrix (CI-built today): 10 transports, 11 database combinations on http, and `ulo generate`
(module, provider, controller on http and rpc, gateway, resolver).

Open questions: Q1 where core tests live (`crates/ulo/tests` with a runtime dev-dependency, or a
`ulo-core-tests` crate like `ulo-codegen-tests`); Q2 whether `examples/` and the wiring-test crate
become workspace members or get CI jobs; Q3 whether `import_cycle`'s "modules may import each other"
is still intended (`graph/cycles.rs` reports import cycles as errors); Q4 whether a per-crate
"ran a test" ledger is wanted (`scripts/ran_a_test.py` is the existing definition); Q5 examples owed
by the six DB, two GraphQL and health crates; Q6 whether `serve_loop_death` is a live guarantee
(it is: `App<Bound>::serve` and the tests named in its row); Q8 `docs/explainers/testing-and-examples.md` and
`crates/ulo/TOWER_COMPAT.md` still describe the master layout.

## Appendix: per-file table

Compact, one row per file or per part of a mixed file. Status: **GAP** (concept survives, no cover:
see section 1), **COVERED** (pinned by the named test), **DROPPED** (concept gone), **UNKNOWN**
(depends on something not found). The "home" hints inside the evidence column are suggestions the
designer may ignore: CORE = `crates/ulo/tests/` (does not exist yet), HTTP = `crates/ulo-http/tests/`,
WS = `crates/ulo-ws-hyper/tests/`, RPC = `crates/ulo-rpc/tests/`, GRPC = `crates/ulo-codegen-tests/tests/`,
IT = `integration-tests/`.

### A1. Examples (`examples/`)

Template coverage, for the redundancy column: `ulo new` writes one greeting handler per transport
(`http`, `ws`, `grpc`, `rpc-{tcp,udp,nats,redis,rabbitmq,mqtt,kafka}`) plus `Greeter`
(`#[injectable]`, `Dep<Greeter>`), `Config::builder().source(File::toml).source(Env::process())`,
`TestApp::of(AppModule).wire()`, `app.serve(shutdown_signal())`, the gRPC and HTTP health modules,
and ten database variants. `scripts/template_matrix.py` builds, tests and clippies each in CI
(`template-matrix` job). Neither the templates nor any other file shows: scopes, a custom extractor,
validation, SSE, rooms, lifecycle hooks, a client, metadata, middleware, tracing, health customisation.

`crates/*/examples/` is empty in every crate today, so the "integration examples live with their
crate" half of `examples/README.md` has no content on this branch either.

| Old file | Demonstrates | Status | Concept in new framework | Redundancy | Evidence / new home |
| --- | --- | --- | --- | --- | --- |
| `build.rs` | compiles `proto/` for the gRPC examples | **GAP** | yes: `ulo-build` | none | follows `grpc_service.rs`; template `grpc/build.rs` is the shape |
| `proto/` | protos for two gRPC examples | **GAP** | yes | `templates/new/grpc/greeter.proto` | travels with the gRPC example |
| `config_module.rs` | typed config from env with defaults, injected anywhere | **GAP** | yes, new API: `ulo-config` `Config`, `Env`, `File`, `Values` | template reads `config/app.toml` + env prefix, but shows no struct with defaults/secrets | `crates/ulo-config/examples/config.rs`; `#[derive(Config)]`, `ConfigModule::<T>::new()` panic-on-failure no longer apply (`ulo-config/src/module.rs`) |
| `config_validation.rs` | `validator` on config at load | **COVERED** | yes (`ulo-config/src/module.rs`, `problems.rs` mention validation; mechanism unverified) | differs from `config_module` by a derive and one attribute | fold two lines into the config example above |
| `custom_extractors.rs` | write a `FromContext` extractor; `Option<T>` turns refusal to `None` | **GAP** | yes, new shape: `ulo_transport::extract::FromCall<T>` | none | `examples/custom_extractor.rs` |
| `extractors_for_request_metadata.rs` | an extractor that can fail and what its error does | **COVERED** | yes | third example of "write an extractor"; the failure half belongs in the example above | fold the error path into `custom_extractor.rs` |
| `extractors_that_compose.rs` | extractor built from extractors; cookie parsing | **COVERED** | yes; no cookie extractor exists in `ulo-http/src/extract` | same concept; cookies are user-land | judgement call J7; fold a three-line composition into `custom_extractor.rs` if `FromCall` allows it (unverified) |
| `derive_injectable.rs` | `#[injectable]` fields, `#[default]`, scope, `#[new]` | **GAP** | yes, new spelling: `#[injectable]`, `#[construct]`, `Dep<T>`, `Singleton`/`PerExecution`/`Transient` (`crates/ulo/src/scope.rs`) | template `greeter.rs` shows the 3-line singleton only | `examples/scopes.rs`: one question, "what does scope change", with assertions on construction counts |
| `provider_patterns.rs` | value/factory/alias/token providers, resolving without a server | **GAP** | yes, new: `m.value`, `m.with`, `m.singleton`, `m.alias`, `m.contribute`, `qualified`, `app.get::<T>()` (`module/def.rs`, `app/handle.rs`) | none | `examples/providers.rs`; the four macros (`provider_value!` etc.) are gone |
| `error_handling.rs` | `#[derive(Error)]`, `Result<T, E>` handlers, canonical envelope, chain handlers | **GAP** | yes: `Classify` derive, `ErrorKind`, `#[ulo::error_handlers]`, `AnyErrorHandler` | none | `examples/error_handling.rs`; shape of the envelope pinned by `ulo-http-conformance` `reshaped`/`recovered` |
| `error_telemetry.rs` | reporting errors off the request path | **GAP** | yes; ADR-0035 pattern still applies (a declining handler observes) | none | `examples/error_reporting.rs`, spawning on `Dep<dyn Runtime>`; low priority |
| `execution_scoped_context.rs` | guard writes a user; a service two levels down reads it | **GAP** | yes: `Ext<T>` (`crates/ulo/src/dependency/ext.rs`), `Extensions::insert`, `PerExecution` | none | `examples/execution_data.rs`; this is the example that shows what an execution is |
| `multi_protocol_context.rs` | one guard/interceptor type running on HTTP, RPC, WS | **GAP** | yes: `AnyGuard<T>`, `AnyInterceptor<T>` (`ulo/src/transport/mod.rs:134`) | partly `ulo-ws-conformance` app guards, none user-facing | `examples/one_guard_every_transport.rs` |
| `file_upload.rs` | `Multipart` extractor | **GAP** | yes: `ulo_http::Multipart` (`extract/body.rs:185`) | none | `examples/file_upload.rs`; no test pins multipart in the new tree (J3) |
| `gateway_http_bridge.rs` | an HTTP handler pushes to WS clients | **GAP** | yes: `Rooms`/`Broadcast` injected into a controller (`ulo-ws/src/rooms.rs`) | `ulo-ws/tests/handoff.rs` `to_all_reaches_every_gateway...` pins the delivery, no example | `examples/http_pushes_to_websocket.rs` |
| `websocket_rooms.rs` | rooms, DMs, broadcast-except-sender | **GAP** | yes: `Rooms`, `RoomsIn`, `.except([id])` | none | `crates/ulo-ws/examples/rooms.rs` (a crate users install deliberately) |
| `websocket_chat.rs` | minimal WS echo | **COVERED** | yes | template `ws/gateway.rs` is the same handler | evidence `crates/ulo-cli/src/templates/new/ws/gateway.rs`, built by `template_matrix.py ws` |
| `websocket_di.rs` | gateway with DI plus global WS guards/interceptors | **COVERED** | yes | `multi_protocol_context` (ported) covers enhancers; DI is the template | `ulo-ws-hyper/tests/messages.rs` `method_guards_interceptors_and_error_handlers_apply_to_their_message` |
| `graceful_shutdown.rs` | hooks, signals, closing WS clients, drain | **GAP** | yes: `AppHandle::close(Signal)`, `Draining`, `app.serve(shutdown)` | template `main.rs` wires `serve(shutdown_signal())` but shows no drain | `examples/graceful_shutdown.rs`, trimmed to drain + one hook; drain semantics pinned by `ulo-ws/tests/handoff.rs` `the_drain_closes_a_same_port_connection_with_going_away` |
| `lifecycle_hooks.rs` | five hooks on providers and modules | **GAP** | yes: `OnModuleInit` etc. traits, `m.on_init(..)` (`ulo/src/hooks.rs`, `module/def.rs`) | none | `examples/lifecycle_hooks.rs`; hooks on non-`HookCapable` scopes are now compile errors |
| `health_checks.rs` | ping/response/custom/memory/disk indicators | **GAP** | yes, new: `Check`, `Health`, `Probe`, `HealthModule` (`ulo-health`), HTTP via `ulo-health-http` | template wires gRPC health only | `crates/ulo-health-http/examples/health.rs`; memory/disk/timeout features unverified |
| `hello_world.rs` | smallest HTTP app | **COVERED** | yes | `ulo new` http template: same controller, module, adapter, built by CI | `templates/new/http/controller.rs` + `common/main.rs`; the single-file form is the one thing lost (J6) |
| `logging.rs` | default stderr subscriber, `RUST_LOG` | **DROPPED** | **gone**: no `try_init` in `crates/ulo` or `ulo-runtime-tokio`; the template calls `tracing_subscriber::fmt::init()` itself | template `common/main.rs` | `ulo-runtime-tokio/tests/own_subscriber.rs` shows the app never installs one |
| `rpc_tracing.rs` | `rpc.request` span per call on tcp/udp/grpc | **GAP** | yes, new owner: `ulo_transport::span::call` + `ulo-otel` | none | `crates/ulo-otel/examples/export_spans.rs` (tracing to OTLP); spans themselves unverified in tests |
| `middleware_examples.rs` | bearer auth, logging, CORS, timeouts, compression, rate limit as reference impls | **GAP** | yes, new: `ulo_http::middleware::Middleware`, `Cors`, `PreDispatch`, tower bridge | none; six concepts in one file | split: `examples/bearer_middleware.rs` (PORT-shaped) and `examples/tower_layer.rs`; CORS is `Cors::new().allow_origin(..)`, one line, shown in docs |
| `route_metadata.rs` | `#[set_metadata]` read by a guard | **GAP** | yes, renamed: `#[ulo::meta]`, `Meta`, `Metadata` (`ulo/src/transport/metadata.rs`) | none | `examples/route_metadata.rs` |
| `rpc_controller.rs` | RPC over tcp: message and event patterns | **COVERED** | yes | template `rpc/controller.rs` (`#[ulo_rpc::message("greeter.greet")]`); event handlers: `ulo-rpc-conformance` `events::reaches_handler` | CI `template-matrix` `rpc-tcp`; thin: no example shows `#[ulo_rpc::event]` (J9) |
| `rpc_udp.rs` | same, udp | **COVERED** | yes | template `rpc-udp` | `template_matrix.py rpc-udp`; wire: `ulo-rpc-udp/tests/conformance.rs` |
| `rpc_nats.rs` | same, nats | **COVERED** | yes | template `rpc-nats` | `template_matrix.py rpc-nats`; `ulo-rpc-nats/tests/conformance.rs` |
| `rpc_nats_client.rs` | calling a remote service via `RpcClient` | **GAP** | yes, new: `RpcClient::request/emit/stream` (`ulo-rpc/src/client.rs`), `RpcClientModule` | no template shows a client | `crates/ulo-rpc/examples/client.rs` on the tcp link (no broker needed) |
| `rpc_streaming.rs` | streamed replies and `RpcClient::stream` | **GAP** | yes: `Items`/stream returns, `StreamCall`, `send_stream`, `duplex` | none | `crates/ulo-rpc-tcp/examples/streaming.rs`; wire pinned by `ulo-rpc-conformance` `streams::*` |
| `grpc_service.rs` | gRPC with DI, guard, interceptor, error handler (254 lines) | **GAP** | yes: `#[ulo_grpc::method]`, `Message<T>` | template covers the bare service; enhancers pinned by `ulo-codegen-tests/tests/enhancers.rs` | `crates/ulo-grpc/examples/service_with_guard.rs`, one enhancer only |
| `grpc_client.rs` | tonic client as a provider | **GAP** | yes, replaced by `GrpcClientModule::for_root(GrpcEndpoint::new(uri))` (`ulo-grpc/src/client.rs`) | none | `crates/ulo-grpc/examples/client.rs`; behaviour: `ulo-codegen-tests/tests/clients.rs` `the_bound_client_calls_its_endpoint` |
| `sse.rs` | counter, named events, ids, broadcaster | **GAP** | yes: `Sse::new`, `.keep_alive`, `.end_event`, `Event::data/json/id/event` (`ulo-http/src/sse.rs`) | none | `examples/sse.rs`, cut to counter + typed events + keep-alive |
| `socket_activation.rs` | adopt a listening socket from a supervisor | **GAP** | yes: `ulo-net` `activation.rs` (`LISTEN_FDS`/`LISTEN_FDNAMES`/`LISTEN_PID`); `BindTarget` gone | none | `examples/socket_activation.rs`; adoption pinned by `ulo-http-hyper/tests/listeners.rs` |
| `validation_complete_guide.rs` | extractors as pipes: `Validated`, defaults, newtypes, guard, interceptor, WS and RPC (431 lines) | **GAP** | yes: `Valid<P>`, `Validate` derive (`ulo-transport/src/validate.rs`); "pipe" framing is gone (ADR-0027) | none; at least four concepts | split to `examples/validation.rs` (`Valid<Json<T>>` + serde defaults); move the NestJS table to docs |

### A2. Integration tests: framework and DI core

Status per row; GAP rows name the new API where known.

| Old file | Tests | Status | Evidence / new home |
| --- | --- | --- | --- |
| `attribute_syntax.rs` | `#[injectable] struct` annotated, not wrapped | **DROPPED** | the old inline form is gone; every template and test uses the annotated struct (`templates/new/common/greeter.rs`) |
| `circular_dependency.rs` | cross-module provider cycle names every provider | **GAP** | CORE `cycles.rs`, asserting the `WiringError` path printed by `graph/cycles.rs`; no test exists |
| `import_cycle.rs` | modules may import each other; provider resolves across | **GAP** | CORE `import_cycles.rs`; `graph/cycles.rs` reports import cycles as errors, so the old claim may be inverted: **check against DESIGN before porting (unverified)** |
| `concurrent_scope.rs` | per-request instances isolated under overlap | **GAP** | CORE `per_execution.rs` using `App::execute` (`app/handle.rs:59`) with `ExecOptions` |
| `execution_scope_sharing.rs` | one instance per execution across guard and controller | **GAP** | CORE `per_execution.rs`; uses `PerExecution` |
| `scope_bubbling.rs` | a controller with a per-request dep is itself per-request | **GAP** | CORE `scope_inference.rs`; new `Auto` scope inference (`scope.rs`, `graph/scopes.rs`) replaces "auto-elevate"; also `#[injectable(singleton)]` refusal |
| `provider_scope.rs` | singleton/request/transient dedupe | **GAP** | CORE `scopes.rs`, merged with `di_core` scope cases |
| `di_core.rs` | singleton built once; transient per injection; `#[inject]`/`#[default]`; config in provider | **GAP** | CORE `scopes.rs` + `injection.rs`; `#[default]`/`#[inject]` field attrs: unverified whether they survive (`#[construct]` replaces?) |
| `di_validation.rs` | which scope may inject which | **GAP** | CORE `scope_rules.rs` for the runtime refusals; the compile-time half (`AllowedIn<S>`) goes to `ulo-codegen-tests/tests/ui` as a compile-fail case |
| `di_providers.rs` | value/factory/alias/token macros | **GAP** | CORE `bindings.rs`: `m.value`, `m.with`, `m.alias`, `qualified`; the four macros no longer exist |
| `provide_macro.rs` | `provide!` auto-detect and markers | **DROPPED** | macro gone (`ulo-macros` exports `construct, error_handlers, guards, injectable, interceptors, meta, module, routes`) |
| `provider_macro_role_detection.rs` | `provider_value!` registers enhancer role by trait impl | **DROPPED** | role detection replaced by typed `m.enhancer::<AnyGuard<Http>>()` (`transport/mod.rs:134`) |
| `marker_free_enhancers.rs` | guard declared by trait impl only | **COVERED** | same: roles are explicit types; `ulo-codegen-tests/tests/enhancers.rs` `an_impl_guard_by_type_reads_the_metadata_of_every_method` |
| `token_format.rs` | one token format across `#[inject]`, factory deps, `resolve` | **GAP** | CORE `keys.rs` over `Key`/`token_of` replacements (`key.rs`, `type_name.rs`); generic and qualified-path spellings |
| `multi_providers.rs` | `multi(Trait)` collects contributions | **GAP** | CORE `contributions.rs`: `m.contribute::<dyn T>()`, `Many<T>` (`dependency/many.rs`) |
| `new_ctor_injection.rs` | `#[new]` constructor with non-stored deps | **GAP** | CORE `construct.rs`: `#[construct]`, `Construct` (`construct.rs`); scopes cases merge into `scopes.rs` |
| `constructor_patterns.rs` | `#[new]` on provider/controller, param injection | **COVERED** | duplicate of `new_ctor_injection`; fold its three shapes into the port above |
| `owned_fields.rs` | `#[inject]` plus `#[default]` plus `Default` fields | **COVERED** | duplicate of `di_core` field cases; fold into `injection.rs` |
| `derive_lifecycle_hooks.rs` | five hooks on `#[injectable]` fire | **GAP** | CORE `hooks.rs`: `OnModuleInit` etc.; hook on a non-`HookCapable` scope is a compile-fail case |
| `lifecycle_hooks.rs` | hook order, controllers' hooks, shutdown hooks once | **GAP** | CORE `hook_order.rs`; shutdown order partly pinned by `ulo-runtime-tokio/tests/app.rs` (close claims), init order not pinned |
| `deferred_instantiation.rs` | module waiting on a global is retried | **GAP** | CORE `global_order.rs`; new loader is `graph/order.rs`; mechanism may differ (unverified) |
| `global_modules.rs` | `global: true` exports reach everyone | **GAP** | CORE `modules.rs`: `m.global()` (`module/def.rs:52`) |
| `modules.rs` | module forms, nested imports, selective exports, negative visibility | **GAP** | CORE `modules.rs` + `visibility.rs` (`graph/visibility.rs`) |
| `module_ref.rs` | strict `ModuleRef` resolution, thread-safe, per-execution refusal | **GAP** | CORE `module_ref.rs` (`module/handle.rs`, `Resolver`); `ExecutionRequired` refusal renamed |
| `module_lookup.rs` | module found by type, by key; ambiguity | **GAP** | CORE `module_lookup.rs` (`App::module`, `module_keyed`) |
| `dynamic_module_controllers.rs` | `DynamicModule` contributes a controller | **GAP** | CORE `dynamic_module.rs` (`module/dynamic.rs`, `m.controller::<C>()`) |
| `resolve_by_hand.rs` | resolve per-execution provider by hand | **GAP** | CORE `execute.rs`: `App::execute`, `ExecOptions`, `Resolver` |
| `request_provider.rs` | `Request` injects without a `providers:` entry | **UNKNOWN** | no framework-registered request provider found in `crates/ulo`; the HTTP request is `HttpCx`/`Ext`. Confirm the concept is gone, then DELETE (unverified) |
| `request_extractor.rs` | `HttpRequest` param in each form | **GAP** | HTTP `handler_forms.rs` (`Request`/`HttpCx` params); the parts-vs-whole distinction may be gone |
| `extension_di.rs` | `Extension<T>` reads a guard's write two constructions down | **GAP** | CORE `ext.rs` with `Ext<T>`/`Option<Ext<T>>` (`dependency/ext.rs`) |
| `extension_bag_reaches_providers.rs` | middleware writes to bag, per-request provider reads | **GAP** | HTTP `middleware_to_provider.rs` (pre-dispatch stage or `Middleware` writes `Extensions`) |
| `extension_bus.rs` / HTTP half (`http_guard_and_middleware_writes_reach_the_handler`, `http_bag_does_not_leak_between_requests`) | bag across middleware, guards, handler | **GAP** | HTTP `extension_bus.rs` |
| `extension_bus.rs` / WS half (`ws_guard_write_reaches_the_handler`) | WS guard write reaches handler | **GAP** | WS `ws-hyper/tests/messages.rs` (already has enhancer cases; add the write) |
| `extension_bus.rs` / body tests (`an_enhancer_reads_the_body_once_and_sees_it_gone`, `a_handler_whose_body_was_taken_is_told_which_extractor_lost`, `a_capturing_stream_reads_the_bag`, `execution_state_survives_until_the_body_is_drained`) | body-taking and stream-held execution | **DROPPED** | taking semantics belong to the old `take_body`; stream/execution lifetime now pinned by `ulo-http/tests/stream_end.rs` and `ulo-http-conformance` `mid_stream` |
| `guard_mut_context.rs` | guard attaches principal, later guard reads | **GAP** | HTTP/GRPC: `ulo-codegen-tests/tests/enhancers.rs` has metadata-reading guards but no write-then-read; add to CORE `enhancer_chain.rs` over a stub transport (`Transport` trait) |
| `common/` (`grpc_stub`, `panics`, `server`, `tracker`, `ws_raw`, `mod`) | shared helpers | **DROPPED** | helpers for deleted/ported tests; replaced by `ulo-conformance-support` (`relay.rs`, `tokio_harness.rs`) and per-crate `tests/support` |
| `main.rs`, `build.rs`, `src/lib.rs`, `Cargo.toml` | harness plumbing | **GAP** | `Cargo.toml` still names `ulo-ws-tungstenite`, tower-compat feature, `ulo-http-axum` etc.; becomes the slim IT crate in section 5 |

### A3. Integration tests: enhancers, errors, pipeline

| Old file | Tests | Status | Evidence / new home |
| --- | --- | --- | --- |
| `enhancers.rs` (`enhancers_execution_order`, `guard_authorization`, `di_in_enhancers`, `app_token_global_enhancers`, `path_qualified_enhancer_attrs`, `stacked_enhancer_attrs_accumulate`) | global > controller > method order; guard then interceptor | **GAP** | order: CORE `enhancer_order.rs` over a stub transport; ADR-0055 (`docs/adr/0055-the-pipelines-order.md`) is the new contract. `app_token_*` and `path_qualified_*` cases: DELETE (concept gone) |
| `enhancer_declaration_order.rs` | entries run in order written across value/token/closure spellings, per transport | **GAP** | CORE `enhancer_order.rs`; `EnhancerSpec::guard_with` replaces closures; only the stub-transport form is needed, the four transport copies are `DELETE`d because `Transport` is generic |
| `enhancer_tokens_resolve_at_create.rs` | misspelled enhancer token fails `create` | **DROPPED** | concept gone: enhancers are typed roles, a missing provider is a `WiringError` (`error/wiring.rs`) |
| `app_token_enhancers.rs` | global enhancer by string token with DI | **DROPPED** | concept gone as above |
| `enhancers_di.rs` | guard, interceptor, middleware are ordinary injectables | **GAP** | CORE `enhancer_di.rs`: `m.enhancer::<AnyGuard<Http>>().provide::<G>()` builds with deps |
| `scoped_enhancers.rs` | per-execution and transient guards/interceptors | **GAP** | CORE `enhancer_scopes.rs`; `EnhancerSpec::guard_with_in`; `ws_execution_scoped_guard_uses_handshake_header` goes to WS `hooks.rs` |
| `guards_build_in_order.rs` | a refusing guard builds nothing below it | **GAP** | CORE `enhancer_order.rs`; commit `4d690533 test(core): pin that a refused call builds nothing below its guard` suggests an earlier home that no longer exists in tree (check `git show 4d690533 --stat`, unverified) |
| `inline_enhancers.rs` | `#[use_guards(Guard{})]` by value on rpc/ws/grpc | **GAP** | GRPC `enhancers.rs` has `an_impl_guard_by_type...`; by-value form: CORE `enhancer_order.rs` (value spelling); the three transport copies DELETE |
| `method_enhancers.rs` | method-level guard/interceptor/handler on ws and rpc | **COVERED** | `ulo-ws-hyper/tests/messages.rs` `method_guards_interceptors_and_error_handlers_apply_to_their_message`; RPC: `ulo-rpc-conformance/src/cases/errors.rs` `guard_refusal`; gRPC: `enhancers.rs` `a_method_guard_refuses_its_method_alone` |
| `global_enhancers.rs` | global > controller > method tiers (HTTP) | **GAP** | HTTP `global_enhancers.rs`, `m.enhancer::<AnyGuard<Http>>()`; same order contract as `enhancer_order` so merge if CORE stub covers tiers |
| `rpc_global_enhancers.rs` | global rpc guard/interceptor/error handler | **COVERED** | `ulo-rpc-conformance/src/cases/errors.rs` `guard_refusal`, `substituted`; global form is the same `m.enhancer::<AnyGuard<Rpc>>()` path as HTTP (unverified) |
| `ws_global_enhancers.rs` | same on ws | **COVERED** | `ulo-ws-conformance/src/cases/close_codes.rs` `guard_refusal`; global registration as above |
| `grpc_global_enhancers.rs` | same on grpc | **COVERED** | `ulo-codegen-tests/tests/enhancers.rs` `an_unscoped_pre_dispatch_entry_sees_a_path_off_the_table` and sibling; global guards on gRPC unverified |
| `guard_rejection_is_an_event.rs` | guard refusal reaches `#[catch(GuardRejection)]` on ws/rpc/grpc | **COVERED** | `GuardRejected` is a core event (`error/mod.rs`); `ulo-rpc-conformance` `errors::guard_refusal`, `ulo-ws-conformance` `close_codes::guard_refusal`, `ulo-codegen-tests/tests/enhancers.rs` `a_method_guard_refuses_its_method_alone` |
| `guard_panic_is_an_event.rs` | panicking rpc/grpc guard goes to chain | **COVERED** | `ulo-rpc-conformance` `errors::panic`; `ulo-codegen-tests/tests/replies.rs` panic cases (`PanicRecovered`, `catch_panic`) |
| `pipeline_panic_is_an_event.rs` | panicking middleware and grpc interceptor reach chain | **COVERED** | `ulo-http-conformance` `errors::recovered`; `ulo-codegen-tests/tests/enhancers.rs` (middleware cases, `ulo_http::middleware::Next`) |
| `panic_recovery.rs` | panics in handler, guard, interceptor, error handler, renderer on HTTP | **COVERED** | `ulo-http-conformance` `errors::recovered`; renderer-panic fallback: unverified, add one case to HTTP `panic_stages.rs` if `DispatchStage` has a renderer stage (`error/mod.rs:540`) |
| `ws_panic_recovery.rs` | ws panic kinds keep connection alive | **COVERED** | `ulo-ws-conformance` `messages::error_envelope` and `close_codes::faulted` |
| `error_handler.rs` | chain claims, falls through, framework events | **GAP** | HTTP `error_chain.rs` for HTTP rendering; `reshaped`/`recovered` cover claim and recover; fall-through-to-default and "chain sees interceptor refusal" are unverified |
| `error_handler_precedence.rs` | method > controller > global claim order | **GAP** | CORE `enhancer_order.rs` (reverse order for error handlers) |
| `error_handler_answers_a_handlers_type.rs` | claim answers empty or an error of its own | **GAP** | folded into CORE `error_handlers.rs`; gRPC half: `ulo-codegen-tests/tests/replies.rs` handlers `substituted`/`rescue` |
| `catch_macro.rs` | `#[catch(T)]` lowering | **DROPPED** | macro gone; `#[ulo::error_handlers]` replaces; `ErrorHandler::handle(err, cx)` downcast cases covered by `ulo-codegen-tests/tests/replies.rs` |
| `error_derive.rs` | `#[derive(Error)]` kinds and defaults | **GAP** | `Classify` derive (`ulo-transport-macros`): `crates/ulo-transport/tests/classify.rs`; no test exists |
| `middleware_error.rs` | `HttpError` from middleware keeps its status | **GAP** | HTTP `middleware.rs` (new `Middleware`, `ulo-http/src/middleware.rs`) |
| `unrouted_is_an_event.rs` | unrouted rpc pattern or ws event claimable | **COVERED** | `ulo-rpc-conformance` `misses::unhandled_pattern`, `unhandled_event`; `ulo-ws-conformance` `messages::unknown_event` |
| `handler_return_is_a_type.rs` | every spelling of a fallible return reaches chain | **GAP** | `ulo-codegen-tests` has handlers returning `Result<_, Refusal>` but no alias spelling; add one to HTTP `handler_forms.rs` and a compile case; `IntoReply<T>` (`ulo-transport/src/reply.rs`) is the contract |
| `extraction_failure_shape.rs` | body of a failed extraction; not seen by chain | **COVERED** | `ulo-http-conformance` `extraction::failures` |
| `metadata_overlay.rs` | impl-block metadata, handler overrides | **GAP** | CORE/WS: `ulo-ws-hyper/tests/messages.rs` `meta_on_a_handler_is_more_specific_than_meta_on_its_impl` covers WS; HTTP/RPC copies DELETE, gRPC `enhancers.rs` covers its own |
| `static_methods.rs` | handler without `&self` | **GAP** | HTTP `handler_forms.rs`; whether `#[routes]` still allows `&self`-less handlers is unverified |
| `async_controllers.rs` | async handler awaited | **GAP** | HTTP `handler_forms.rs`; the HTTP conformance app's handlers are sync (`fn hit`), so no HTTP async handler is pinned |

### A4. Integration tests: HTTP surface

| Old file | Tests | Status | Evidence / new home |
| --- | --- | --- | --- |
| `http_e2e.rs` | async handlers, config injection, singleton and per-request controllers, optional extractor, JSON body, extensions | **COVERED** | each claim has a narrower home above (scopes, extractors, `ext`); the "hold together through one app" claim is now `templates/new/http` built and tested by CI |
| `global_chain_conformance.rs` | pre-routing chain observes 404/405/matched/rewrite, short circuit | **COVERED** | `ulo-http-conformance` `pre_dispatch::preflight`, `routing::{not_found,method_not_allowed}`; `PreDispatch` stage (ADR-0056). Rewrite of URI changes route: unverified |
| `trailing_slash_conformance.rs` | `/app` equals `/app/`, query kept, root | **GAP** | add a `trailing_slash` scenario to `ulo-http-conformance/src/cases/routing.rs`; `router/pattern.rs` states the rule, no test exists |
| `param_syntax_conformance.rs` | `{param}` extracted; 405 vs 404 on param paths | **GAP** | scenario in `ulo-http-conformance` routing cases (`Page`/`Query` exist, no path-param route) |
| `cors.rs` | preflight, decorated actuals, allow-list with credentials, max-age | **GAP** | `ulo-http-conformance` `preflight` pins one preflight against the app's one `Cors` entry; the other five cases go to HTTP `cors.rs` (`ulo-http/src/cors.rs`) |
| `tower_compat.rs` | tower layers: headers, CORS, body round trip, extensions, composition, compression | **GAP** | HTTP `tower_bridge.rs` for `ulo_http::tower_bridge::Service`; `TOWER_COMPAT.md` in `crates/ulo` is stale (names `tower-compat` feature) |
| `extractors.rs` | query, json, validated, typed path, aliased body | **GAP** | HTTP `extractors.rs`; `ulo-http-conformance` `extraction::failures` pins only the failure shape. Aliased-body case DELETE (name-matching macros gone, ADR-0040) |
| `attribute_extractors.rs` | `#[body]`/`#[param]`/`#[query]` markers | **DROPPED** | markers do not exist in `ulo-http-macros` (verbs only: get, post, put, patch, delete, head, options) |
| `body_and_multipart_extractors.rs` | `Body<T>` by content type; multipart round trip | **GAP** | HTTP `body_extractors.rs`: `Json`, `Form`, `Multipart` (`extract/body.rs`); `Body<T>` content-type sniffing is gone (unverified) |
| `body_already_read.rs` | second body reader told body is gone | **DROPPED** | `take_body`/`CONSUMES` are not in the new tree (grep of `ulo-http/src/extract` finds neither); verify the replacement is a type-level or `Request`-ownership rule (unverified) |
| `streaming.rs` | request body reaches handler as a stream | **GAP** | HTTP `body_stream.rs`: `BodyStream` (`extract/body.rs:132`); backend buffering differs by host (actix 256 KiB), so keep to the Keeper backend |
| `request_extractor.rs` | see section 2 | — | listed there |
| `validated_transports.rs` | `Validated<Payload<T>>` on rpc and ws | **GAP** | `Valid<P>` is cross-transport (`ulo-transport/src/validate.rs`); one test each in `ulo-rpc/tests/` and `ulo-ws-hyper/tests/messages.rs`; no pin today |
| `sse.rs` (wire: `test_sse_headers`, `test_sse_basic_wire_format`, `test_sse_event_fields`, `test_sse_multiline_data`) | headers and framing | **GAP** | HTTP `sse_wire.rs`; `ulo-http-conformance` `sse::error_event` pins only the error event; `Event::data/id/event/retry/comment` framing is unpinned |
| `sse.rs` (handler forms: `test_sse_fallible_stream`, `test_sse_attr_macro_*`, `an_aliased_boxed_stream_streams`, `fallible_setup_*`) | stream and `Result` return spellings | **DROPPED** | `#[sse]` macro gone; `Sse::new(stream)` with `SseItem` is one form (`ulo-http/src/sse.rs:207`); error path: `error_event` |
| `sse.rs` (`test_sse_broadcaster_delivers_to_subscriber`) | broadcaster pattern | **COVERED** | user-land; the example covers it |
| `sse_adapter_conformance.rs` | each adapter serves endless SSE; refusal for collecting adapters | **COVERED** | `ulo-http-conformance` app route `endless` with `Probe`/`Timer`, run by every `crates/ulo-http-*/tests/conformance.rs`; refusal-by-declaration is in `EmbedLimits` (`Host::limits`) |
| `cancellation.rs` | disconnect drops handler future; body drop cancels feeding work | **COVERED** | `ulo-http-conformance/src/cases/disconnect.rs` `mid_stream`; `ulo-http/tests/post_deadline_body.rs`, `stream_end.rs`; `dropped.rs` in each adapter crate |
| `graceful_shutdown.rs` | `close()` closes WS, stops HTTP, runs destroy hook | **COVERED** | `ulo-http-conformance` drain (`http1`, `http2`, `goaway`), `ulo-ws/tests/handoff.rs` `the_drain_closes_a_same_port_connection_with_going_away`, `ulo-runtime-tokio/tests/app.rs`; one cross-transport version stays in IT (section 5) |
| `serve_loop_death.rs` | a dead serve loop closes the app | **COVERED** | `App<Bound>::serve` (`crates/ulo/src/app/mod.rs` ~498-559) closes the app when a serve ends; pinned by `crates/ulo-runtime-tokio/tests/app.rs` :1187, :1192 and :1252 and by `crates/ulo/tests/serve_discard.rs` (`CORE_TEST_PLAN.md` section 3 records the same) |
| `bind_refusals.rs` | refuse-or-none bind: failing transport fails bind and releases sockets; no-adapter declarations refused | **GAP** | IT `bind_refusals.rs` for the first two cases (two real transports); the three "no adapter" cases DELETE, controllers without a server are now `StartupError::Configure` (`error/mod.rs:37`) — pin one in CORE `startup_errors.rs` |
| `bind_phase_order.rs` | all registration before any socket | **GAP** | folded into IT `bind_refusals.rs`; per-server ordering: `ulo-ws-hyper/tests/prepare.rs` |
| `adapter_error_contract.rs` | adapter error survives as `source` | **GAP** | CORE `startup_errors.rs` with a stub `Server`; `StartupError::Bind { transport, source: Redacted }` is the new shape |
| `an_application_is_send.rs` | app crosses threads; serves from spawned task | **GAP** | CORE `send.rs` as a compile-time `assert_send`, plus one spawned `serve`; `ulo-runtime-tokio/tests/claims.rs` races `close` across threads only |
| `bind_target_conformance.rs` | each adapter adopts a caller socket by address identity | **COVERED** | `ulo-http-hyper/tests/listeners.rs`, `ulo-listener-tokio/tests/listener.rs`, `ulo-listener-smol/tests/listener.rs`; per-adapter: embedded hosts take the hyper listener (`ulo-http-axum/tests/conformance.rs`). Rocket refusal: unverified |
| `bind_target_handoff.rs` | connection made between generations is answered | **COVERED** | `ulo-http-hyper/tests/listeners.rs` (socket handed down by `LISTEN_FDS`, adopted by name); the kernel-queue claim has no direct pin (unverified) |

### A5. Integration tests: WebSocket

| Old file | Tests | Status | Evidence / new home |
| --- | --- | --- | --- |
| `ws_adapter_conformance.rs` | close handshake, ping/pong, fragments, own close, `Refused` close | **COVERED** | `ulo-ws-conformance` `close_codes::{refused_with,own_code}`, `keep_alive::pings`, `messages::control_frame`; fragment reassembly: unverified (likely in the `async-tungstenite` layer) |
| `websocket_e2e.rs` (`websocket_echo_end_to_end`, `websocket_bare_gateway_accepts_connection`, `websocket_on_connect_without_subscriptions_fires`) | echo, bare gateway, `on_connect` | **COVERED** | `ulo-ws-conformance` `messages::round_trip`, `handshake::as_declared`, `hooks::on_connect_refusal`; `ulo-ws-hyper/tests/attributes.rs` |
| `websocket_e2e.rs` (`websocket_broadcast_end_to_end`, `gateway_injected_into_rest_controller`) | broadcast; gateway injected into HTTP controller | **COVERED** | `ulo-ws/tests/handoff.rs` `a_room_broadcast_reaches_the_other_members_and_not_the_sender`, `the_http_port_serves_a_gateway_and_a_route_side_by_side` |
| `websocket_e2e.rs` (`websocket_separate_port_end_to_end`, `separate_port_close_stops_ws_server`, `gateway_port_zero_binds_separately_from_http_port_zero`, `a_keepalive_ping_...`) | own-port gateway, port 0, keepalive | **COVERED** | `ulo-ws-hyper/tests/handshake.rs`, `limits.rs`; `ulo-ws-conformance` `keep_alive::{pings,pong_timeout}`; `ulo-ws/tests/handoff.rs` `a_gateway_declared_for_its_own_port_is_not_served_on_the_http_port` |
| `websocket_broadcast.rs` | broadcast to all, room, client; leave; module provides service | **COVERED** | `ulo-ws/tests/broadcast.rs`, `handoff.rs` `to_all_reaches_every_gateway...`, `a_connection_that_left_the_room_receives_none_of_its_broadcasts`, `a_client_broadcast_reaches_only_the_connection_it_names` |
| `ws_connect_execution.rs` | connect guard's write reaches `on_connect` | **GAP** | WS `hooks.rs` addition; `session_t_starts_from_default_per_connection_and_is_the_one_the_guards_saw` (`handshake.rs`) covers session, not the bag |
| `ws_connect_refusal.rs` | refusal told policy; panicking guard 1011; connect runs no interceptors | **COVERED** | `ulo-ws-conformance` `close_codes::{guard_refusal,faulted}`, `hooks::on_connect_refusal`; interceptors-not-run-for-connect: unverified |
| `ws_execution_scope.rs` | per-execution provider is rebuilt per message | **GAP** | WS `messages.rs`; execution scope per message is a core claim with no pin (needs `PerExecution` in the WS conformance app, `app.rs`) |
| `ws_extractors.rs` | handler takes payload/client in any order, or nothing | **COVERED** | `ulo-ws-hyper/tests/attributes.rs` (handler per reply kind), `ulo-ws-conformance` `messages::round_trip`; order-freedom via `Param<T, M>` (`ulo-transport/src/__private.rs`) unverified |
| `ws_handler_stream.rs` | stream handler delivers all items in order | **COVERED** | `ulo-ws-conformance` `messages::reply_stream`; `ulo-ws-hyper/tests/attributes.rs` `stream_reply_writes_each_item_then_completes` |
| `ws_listener_adoption.rs` | separate-port WS on caller socket | **DROPPED** | `ulo-listener-*` tests and `ulo-ws-hyper/tests/handshake.rs`; `use_websocket_listener` no longer exists (`grep` finds no such method) |
| `ws_session.rs` | session established by connect guard, per connection, on teardown | **COVERED** | `ulo-ws-hyper/tests/handshake.rs` `session_with_builds_one_session_per_connection_from_the_upgrade_request`, `hooks.rs` `the_drain_is_drain_and_on_disconnect_still_runs_and_reads_the_session`; `declared_metadata_reaches_a_ws_handler`: PORT note above |
| `ws_tail.rs` | abandoned ws stream fires cancel; drained does not | **COVERED** | `ulo-ws-conformance/src/cases/stream_end.rs` `call`, `written`, `discarded`; `ulo-ws-hyper/tests/stream_end.rs` |
| `graphql_subscriptions.rs` | graphql-ws handshake, subscribe/deliver/complete, ping, 4406, per-subscription cancel | **GAP** | `ulo-graphql-ws` has only `tests/runtime.rs` (task spawning); the protocol cases move to `crates/ulo-graphql-ws/tests/protocol.rs` (a real client on `ulo-ws-hyper`); also the cross-crate piece for IT (below) |

### A6. Integration tests: RPC

| Old file | Tests | Status | Evidence / new home |
| --- | --- | --- | --- |
| `rpc_tcp.rs` / panics (`rpc_handler_panic...`, `rpc_guard_panic...`, `rpc_interceptor_panic...`, `rpc_error_handler_panic...`, `rpc_renderer_panic...`) | panics keep connection alive | **COVERED** | `ulo-rpc-conformance` `errors::panic`, `recovery::after_disrupt` |
| `rpc_tcp.rs` / lifecycle (`tcp_app_shutdown_stops_the_accept_loop`, `tcp_in_flight_request_completes_during_drain`, `tcp_drain_aborts_after_timeout`) | shutdown and drain | **COVERED** | `ulo-rpc-conformance` `drain::{window,times_out_during_drain}`; `ulo-rpc-tcp/tests/drain_end.rs`; `ulo-rpc/tests/drain_window.rs` |
| `rpc_tcp.rs` / `tcp_backpressure_rejects_excess_and_releases_after_completion` | max in flight | **COVERED** | `ulo-rpc-conformance` `admission::over_the_bound` |
| `rpc_tcp.rs` / payload (`typed_payload_round_trip_succeeds`, `typed_payload_parse_failure_...`, `an_explicit_single_output_round_trips`) | typed payload, undecodable | **COVERED** | `ulo-rpc-conformance` `unary::round_trip`, `errors::undecodable_payload` |
| `rpc_tcp.rs` / `tcp_client_metadata_reaches_handler`, `declared_metadata_reaches_an_rpc_handler` | headers, metadata | **COVERED** | `ulo-rpc-conformance` `headers::reach_call_headers`; declared metadata: `ulo-ws-hyper` `meta_on_a_handler...`, RPC unverified |
| `rpc_tcp.rs` / extractors (`a_handler_takes_only_the_payload`, `rpc_extractors_compose_in_any_order`, `an_rpc_handler_can_take_nothing`, `an_empty_patterns_impl_registers_no_patterns`) | handler parameter forms | **COVERED** | `ulo-rpc-conformance` `unary::context_parameter`, `round_trip`; empty impl: unverified |
| `rpc_tcp.rs` / scopes and writes (`rpc_guard_write_reaches_the_handler`, `a_request_scoped_provider_is_shared_within_one_rpc_call`, `a_request_scoped_rpc_controller_is_built_per_call`, `a_singleton_rpc_controller_is_built_once`, `an_rpc_controller_elevates_to_request_scope`) | scope and bag over RPC | **COVERED** | the scope claims are CORE's (ported in section 2); RPC shares the same `Construct`/scope path, so a per-transport copy adds nothing |
| `rpc_tcp_stream.rs` | frames in order, binary base64, mid-stream errors, bag across drain, cancel frame, dropped connection | **COVERED** | `ulo-rpc-conformance` `streams::{server_stream,client_stream,bidi_stream}`, `cancel::mid_stream`, `payloads::binary`; `ulo-rpc-tcp/tests/interop.rs`, `dropped_link.rs` |
| `rpc_udp.rs` | round trip, unknown pattern, panic, fire-and-forget, shutdown, retries, oversize, drain | **COVERED** | `ulo-rpc-udp/tests/conformance.rs`, `drain_end.rs`, `dropped_link.rs`, `close_read.rs`; `ulo-rpc-conformance` `payloads::oversized`, `misses::unhandled_pattern` |
| `rpc_udp_stream.rs` | datagram stream grammar, oversize item, cancel datagram | **COVERED** | `ulo-rpc-conformance` `streams`, `payloads::oversized`, `cancel::mid_stream` run on udp; udp stream-item-too-large case: unverified |
| `rpc_binary_payload_is_refused.rs` | tcp/udp refuse `Binary` before I/O | **COVERED** | `ulo-rpc-conformance` `payloads::binary` (tcp CBOR variant: `ulo-rpc-tcp/tests/conformance_cbor.rs`); "no I/O before refusal" unverified |
| `rpc_controller_injection.rs` | an RPC controller is not injectable | **UNKNOWN** | controllers register via `m.controller::<C>()`, a separate binding kind; whether injecting one is refused with a `WiringError` is unpinned: decide in CORE `controllers.rs` |
| `rpc_global_enhancers.rs`, `unrouted_is_an_event.rs` | see section 3 | — | listed there |

### A7. Integration tests: gRPC

| Old file | Tests | Status | Evidence / new home |
| --- | --- | --- | --- |
| `rpc_grpc_macros.rs` / call shapes (`grpc_service_macro_di_round_trip`, server/client/bidi streaming) | four call shapes | **COVERED** | `ulo-codegen-tests/tests/grpc.rs` `unary_answers_the_message_and_its_reply_metadata`, `server_streaming_answers_each_message`, `client_streaming_reads_every_message`, `bidi_answers_each_message_as_it_arrives` |
| `rpc_grpc_macros.rs` / enhancers (guard accept/reject, method stacking, interceptor, error handler) | enhancers on gRPC | **COVERED** | `ulo-codegen-tests/tests/enhancers.rs`, `replies.rs` handlers |
| `rpc_grpc_macros.rs` / panics (handler, guard, interceptor, error handler) | panic surfaces as `Internal` | **COVERED** | `ulo-codegen-tests/tests/replies.rs` and `enhancers.rs` (`PanicRecovered` cases) |
| `rpc_grpc_macros.rs` / `grpc_drain_timeout_aborts_long_running_streams` | drain ends in-flight streams | **COVERED** | `ulo-codegen-tests/tests/builtins.rs` `health_turns_not_serving_when_the_drain_begins`; stream-ending-at-drain: unverified |
| `rpc_grpc_macros.rs` / `grpc_backpressure_rejects_excess_and_releases_after_completion` | max in flight | **UNKNOWN** | `Admission` exists in `ulo-transport` and `ulo-grpc/src/server.rs`; no gRPC test found. Port into GRPC `admission.rs` or add a gRPC case to `ulo-rpc-conformance`-style admission (unverified) |
| `rpc_grpc_macros.rs` / scope and metadata (`a_request_scoped_grpc_service_is_built_per_call`, `a_singleton_grpc_service_is_built_once`, `declared_metadata_reaches_a_grpc_guard`, `grpc_guard_write_reaches_the_handler`) | scope, metadata | **COVERED** | metadata: `enhancers.rs` `an_impl_guard_by_type_reads_the_metadata_of_every_method`; scope is CORE's |
| `rpc_grpc.rs` | adapter round trip against tonic-health | **COVERED** | `ulo-codegen-tests/tests/builtins.rs` `health_reports_serving_for_every_known_service_and_the_server` |
| `rpc_grpc_listener_adoption.rs` | tcp/udp/grpc adopt caller sockets | **COVERED** | `ulo-listener-tokio/tests/listener.rs`, `ulo-rpc-tcp/tests/client_address.rs` (address identity unverified for udp/grpc) |
| `grpc_add_service_warns.rs` | `add_service` bypass is warned | **DROPPED** | `add_service` has no counterpart in `ulo-grpc/src` (grep empty); reflection and health are built-ins (`builtins.rs`) |
| `grpc_client_injection.rs` | tonic client as a provider | **COVERED** | `ulo-codegen-tests/tests/clients.rs` `the_bound_client_calls_its_endpoint`, `connect_does_no_network_io_and_the_first_call_finds_the_endpoint_down` |
| `grpc_deadline.rs`, `grpc_deadline_fires_the_token.rs` | `ctx.deadline()`; deadline cancels execution | **COVERED** | `ulo-codegen-tests/tests/deadlines.rs` (e.g. `a_call_answered_before_its_deadline_reads_the_deadline_it_was_given`, `an_unclaimed_deadline_answers_deadline_exceeded_and_drops_the_handler_as_deadline`) |
| `grpc_domain_error_reaches_the_chain.rs`, `grpc_interceptor_refusal_reaches_the_chain.rs`, `grpc_error_lift.rs`, `grpc_status_names_its_code.rs` | domain error to chain and to status code | **COVERED** | `ulo-codegen-tests/tests/replies.rs` (`rescue`, `substituted`, `lookup`), `grpc.rs` `unary_error_answers_its_kind_as_the_status_code`; naming a code outside the kind table: unverified |
| `grpc_enhancers_answer_with_the_reply.rs` | enhancer reads, stamps, replaces the reply | **COVERED** | `enhancers.rs` `method_and_impl_interceptors_both_write_the_reply_headers`; replacement by another type: `compile_fail.rs` ui cases `grpc_reply_of_another_message` |
| `grpc_global_enhancers.rs` | see section 3 | — | listed there |
| `grpc_handler_form.rs` | own reply/error types, streaming handler, alias, bag, guard reads message | **COVERED** | `grpc.rs`, `replies.rs` (`watch`, `watch_or_fail`, `watch_or_status`), `ui/grpc_*` compile-fail |
| `grpc_manual_trait_form.rs` | manual (no-proto) trait with named stream type | **DROPPED** | concept gone: method shapes come from `Message<T>` and `#[ulo_grpc::method(pb::svc::Method)]`, checked by `ui/grpc_*_of_another_message` |
| `grpc_metadata_and_status_details.rs` | repeated/binary metadata; `grpc-status-details-bin` | **UNKNOWN** | `GrpcMetadata` is pinned by `replies.rs` `whoami`; the status-details trailer (`ulo-grpc/src/status.rs`) has no test found; add to GRPC `status_details.rs` (unverified) |
| `grpc_reflection.rs` | reflection lists services | **COVERED** | `builtins.rs` `reflection_v1_lists_the_descriptor_set_s_services_and_finds_a_symbol_s_file`, `reflection_v1alpha_...` |
| `grpc_service_injection.rs` | a gRPC service is not injectable | **UNKNOWN** | same question as `rpc_controller_injection.rs` |
| `grpc_tail.rs` | abandoned streaming reply fires token; drained does not | **COVERED** | `ulo-codegen-tests/tests/stream_end.rs` (`a_stream_the_caller_abandons_before_its_trailers_reports_cut_off`, `a_written_stream_reports_completed_at_its_trailers`) |
| `grpc_tls.rs` | serve over TLS; unreadable cert fails bind | **UNKNOWN** | `ulo-grpc/src/server.rs` mentions TLS, `ulo-net/tests/tls.rs` tests the TLS config, but no gRPC TLS call test exists; port to GRPC `tls.rs` using `ulo-test-certs` (unverified) |

### A8. Integration tests: GraphQL, config, DB

| Old file | Tests | Status | Evidence / new home |
| --- | --- | --- | --- |
| `graphql_juniper_mount.rs`, `graphql_module_identity.rs`, `graphql_service_injection.rs` | mount, export service, module identity by config fingerprint | **GAP** | `crates/ulo-graphql-juniper/tests/mount.rs`, `crates/ulo-graphql-async-graphql/tests/mount.rs`; one shared suite via the `Engine` trait (`ulo-graphql/src/lib.rs:23`) is the better shape (two implementors); identity is now `ModuleIdentity` + `Keyed` (`module/keyed.rs`) |
| `graphql_playground_is_html.rs` | playground served as `text/html` | **GAP** | same shared suite, `ulo-graphql-http` route |
| `config_injection.rs` | `ConfigService<T>` from env, defaults, missing value names the type | **GAP** | `crates/ulo-config/tests/` (new `load.rs`); `secret_leak.rs` and `yaml_alias_bomb.rs` pin hostile input only. `ConfigService`, derive `Config`, `#[env]` are gone |
| `provider_startup_check.rs` | DynamicModule provider reports failed startup check naming module | **COVERED** | `ulo-db-conformance` `connect::unreachable_fails_at_the_bound`, `connect::bad_credentials_are_permanent`; core: `ConnectError::Readiness` |

### A9. Integration tests: `coverage_ledger.rs`

| Old file | Tests | Status | Evidence / new home |
| --- | --- | --- | --- |
| `coverage_ledger.rs` / `the_ledger_is_complete`, `the_ledger_matches_the_tree`, `every_hole_is_still_a_hole`, `every_support_crate_is_used_and_untested`, `every_suite_file_is_declared`, `every_suite_file_says_what_it_proves` | every crate under `crates/` is proved somewhere; three-column `Proof` (suite/own/unit); entries checked both ways | **GAP** | The question still has an owner-less answer: nothing in CI states that each crate runs a test (`ci.yml` has `test`, `smol`, `brokers`, `miri`, `template-matrix`, none of which asserts per-crate presence). But the listing is stale in kind: `suite` meant "registered in `integration-tests`", and that column is now false for almost every crate (the proof lives in five `*-conformance` crates and per-crate `tests/`). It also names `ulo-db-prisma`, removed in `8726c104`. New shape: a `scripts/coverage_ledger.py` beside `ran_a_test.py` that reads `cargo metadata`, counts `ran_a_test` lines per crate from one `cargo test --workspace` run, and checks each crate without one against a `HOLES` list with a written reason. The reason text is the part worth keeping. Today's actual holes, from section 0, would populate it (Q4) |
| `coverage_ledger.rs` / `every_example_is_indexed`, `every_example_is_runnable` | every example appears in `examples/README.md`; every file is declared in `examples/Cargo.toml` | **GAP** | `examples/` is excluded from the workspace and `ci.yml` never builds it, so these two tests are the only thing that would notice a broken example. New owner: a CI step `cargo check --manifest-path examples/Cargo.toml --examples` plus the README index check in the same script; or make `examples` a workspace member (Q2) |

