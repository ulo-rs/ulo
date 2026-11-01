# Salvage: the pipeline (guards, interceptors, error chain, panic recovery, middleware)

## 1. Header

**Subject.** The 2026-09-20/21 pipeline audit against the old framework at master `654c7183`:
middleware, guards, interceptors, error handlers and the chain, panic recovery, execution order and
tier stacking, across HTTP, RPC, WebSocket and gRPC. It filed F121–F137, corrected F60, settled
D1–D5 plus an API pass, an enhancer grammar and three further items, and proposed ADR-0055 (*the
pipeline's order*) and ADR-0056 (*the pre-dispatch role*).

**Sources read in full** (`docs/experiments/di-redesign/salvage/source/pipeline/`):

| File | Words |
| --- | --- |
| `PIPELINE_AUDIT.md` | 4890 |
| `PIPELINE_AUDIT_HANDOFF.md` | 779 |
| `PIPELINE_BUILD_HANDOFF.md` | 1672 |

Also read for status: `FRAMEWORK_GAPS.md` entries F50, F51, F57, F60, F73, F74, F81, F84, F88,
F118–F137, F242 (F56 has no entry left in the ledger; its closure is recorded in BUILD_SEQUENCE row
27); `BUILD_SEQUENCE.md` rows 26–30, 33, 55–63, 81, 101, triage items S5, S6, S13 and the row notes
for 26–30; `docs/adr/0055-the-pipelines-order.md`, `docs/adr/0056-the-pre-dispatch-role.md`; core
`DESIGN.md` §0, §3.7, §3.8, §7; `transports/DESIGN.md` §2.4, §3.3, §3.4, §6.2; and the code cited
below.

**Date** 2026-11-01. **Tip** `108e406f` on `experiment/di-redesign` (one CI-allowlist commit past
`331c35b9`; no pipeline file changed).

**Ancestry that decides several verdicts.** The branch's merge base with master is `3cc3651`, which
contains PIPE-ADR (#364) and PIPE-1..4 (#365–#368), merged on master 2026-09-26. The rebuild then
replaced the old `crates/ulo` pipeline outright: `MiddlewareConsumer`, `configure_middleware`,
`APP_GUARD`, `APP_MIDDLEWARE`, `safe_render` and `closure_leaf` return no match under `crates/`
(`grep -rn` over `*.rs`). The PIPE-1..4 code itself is therefore gone; what survives is what the
rebuild re-implemented in `crates/ulo/src/transport/{pipeline,enhancer,next,mod}.rs` and the HTTP
and gRPC pre-dispatch stage in `crates/ulo-http/src/pre_dispatch.rs` and
`crates/ulo-grpc/src/pre_dispatch.rs`.

## 2. Summary

| Status | Count |
| --- | --- |
| built | 34 |
| superseded | 27 |
| open | 10 |
| dropped | 9 |
| UNVERIFIED | 1 |
| **total** | **81** |

A row with two halves is counted by its first status. "Open" counts a live gap, an unanswered
question or a missing test; "superseded" includes the built-differently-with-a-loss cases (D2, D4),
each flagged in its row. No open item is a behaviour defect against the rebuild's own design; OP-1
is a design gap with a security shape.

**The five most valuable open items** (OP-3 is a superseded decision whose record is open).

1. **OP-1 / F118: a route-scoped pre-dispatch entry does not run on a miss**, so
   `apply_for::<ApiKeyAuth>(["/admin/*"])` answers an anonymous caller 404 for an unrouted
   `/admin/x` and 401 for a routed one: the route table under the scope is enumerable without
   credentials. Pinned as intended behaviour by `crates/ulo-http/tests/middleware.rs:225`. Not in
   the ledger.
2. **OP-2 / D5: ADR-0055 and ADR-0056 sit in `docs/adr/` as `proposed` and contradict the
   rebuild on at least eight points** (guard answer type, `Transport` sealed, error-handler
   argument order and name, `HasHeaders`, per-transport error enums, pre-dispatch on four
   transports, refusing a non-singleton middleware, the closure's DI route). Listed in the
   README index (`docs/adr/README.md:70-71`).
3. **OP-3 / D4: the rebuild adopted the guard answer ADR-0055 rejects.** `Guard::can_activate`
   answers `Result<bool, BoxError>` (`crates/ulo/src/transport/mod.rs:81-83`); ADR-0055's *Roads
   not taken* rejects `Result<bool, E>` as "two refusal paths that render differently". The
   rebuild assigns the two paths meanings (403 known caller, 401 unauthenticated), which answers
   the rejection's argument, but no record says so.
4. **OP-4: the claim that an outer pre-dispatch entry decorates the answer an inner entry's `Err`
   became (CORS headers on a 401) has no test.** `transports/DESIGN.md` §3.3 states it; the
   middleware and CORS suites pin a miss and a 405 being decorated
   (`crates/ulo-http/tests/cors.rs:207`, `crates/ulo-http/tests/middleware.rs:205`) and pin
   nothing for an inner entry's failure or for an error handler's claimed answer. This is the probe
   the audit itself owed and never ran (build handoff, PR 12 verification).
5. **OP-5 / F137 residue: the API pass's "context first" and `catch` naming were not adopted.**
   `ErrorHandler::handle(&self, err, cx)` keeps the error first and the name `handle`
   (`crates/ulo/src/transport/mod.rs:103-105`), while `Guard` and `Interceptor` take `cx` first.
   A designer call, not a defect.

## 3. Items

### 3.1 Order and laziness (memo §1, §3; D1)

| ID | What | Status | Evidence at HEAD | Value to extract |
| --- | --- | --- | --- | --- |
| F121 | Spelling (token vs inline) decided run order ahead of declaration order and tier; error handlers ignored declaration order | built | One `Vec<Decl<R>>` per role per tier (`crates/ulo/src/transport/enhancer.rs:28-32`); the macro emits one statement per entry in written order, an impl-level value at its position (`crates/ulo-macros/src/enhancers/mod.rs:183-218`); pinned by `crates/ulo/tests/enhancers.rs:90` (`g1, g2, c1, TypedC2, c3, m1, m2, TypedM3`) | The security shape is the reason to keep a permutation test: `#[use_guards(Authenticate, RoleCheck::new(..))]` and `#[use_guards(Authenticate::new(..), RoleCheck)]` ran the two guards in opposite orders (F121). The rebuild's test pins one interleaving; the audit's falsification (permute the spellings, assert the permuted order) is not in the suite. |
| D1 | "A declaration's order is the contract" | built | as F121 | Rejected alternative, kept: documenting the split, because a caller would need to know which spelling was used to know which guard runs first (memo §10 D1). |
| F122 | Every guard and interceptor built before the first guard runs | built | `admit` resolves each global entry lazily and checks it before the next (`crates/ulo/src/transport/pipeline.rs:180-190`); interceptors built only in `respond`, after admission (`:200-222`); `Resolver::entries` documented as the lazy walk (`crates/ulo/src/resolver.rs:191-199`); pinned by `crates/ulo/tests/enhancers.rs:174` (`build g1, guard g1, build r1, guard r1`) | Probe result worth keeping as the regression shape: a refused request logged `built:guard0, built:guard1, built:interceptor, ran:guard0` at `654c7183` (memo §3). DESIGN §7 step 2 records why `Many<AnyGuard<T>>` is not used: it builds every guard first. |
| F122-perf | F122 and F57 are allocation-shaped and were never measured (memo §11) | open | `build_interceptors` allocates a `Vec` per call (`pipeline.rs:213-222`); `claim` collects the global error handlers into a `Vec` per failure (`:273-276`). Not in the ledger as a cost item. | No finding in the old repo was filed on cost alone; the performance subject decides whether these matter. Pointer only. |

### 3.2 Where a failure is shaped (memo §2, §5, §9; D3)

| ID | What | Status | Evidence at HEAD | Value to extract |
| --- | --- | --- | --- | --- |
| F123 | Module middleware's error stringified into `MiddlewareFailure` | superseded | No module middleware exists (`transports/DESIGN.md` §3.3, "There is no per-module middleware"). A pre-dispatch entry's `Err` reaches the error handlers as the `BoxError` it was (`crates/ulo-http/src/pre_dispatch.rs:11-13`, `:644`), so a handler can downcast the domain error | The probe pair that proved the loss was the path's, not the error's: the same `DomainBoom` raised from the handler arrived typed, from the middleware as `MiddlewareFailure` (F123). Reusable as a test: one domain error raised from a pre-dispatch entry and from a handler, one handler downcasting it, both claimed. |
| F124 | Middleware `Err(HttpError)` rendered without the chain | superseded | "no middleware renders its own errors" (`transports/DESIGN.md` §3.3); `an_entrys_failure_reaches_the_global_error_handler_with_its_kind` (`crates/ulo-http/tests/middleware.rs:269`) | The stated reason in the old code ("the user constructed the wire shape themselves") was refuted by the observation that a handler constructs the same shape and was offered to the chain regardless (F124). |
| F125 | Global middleware failure: literal 500, no chain, status lost | built | Unscoped and scoped sub-steps both run inside the error chain; on a miss only global handlers apply (`pre_dispatch.rs:11-13`); `an_entry_that_fails_is_rendered_by_the_kind_it_failed_with` (`middleware.rs:252`) pins 401 with the error's own challenge, 403, and 500 with the text withheld | The authentication case is the motivating one: a globally registered auth middleware could not answer 401 (F125). |
| F81 | A panic in global middleware recovered then dropped | built | `DispatchStage::PreDispatch` (`crates/ulo/src/error/mod.rs:600-602`); entries run inside `AppHandle::catch_panic` and the panic is offered as `PanicRecovered` (`pre_dispatch.rs:500-505`, `transports/DESIGN.md` §3.3) | None beyond F125. |
| S9-gate | ADR-0052 clause: a miss "runs the global interceptors" struck; a miss reaches global error handlers only | built | `ulo::recover(None, ..)` offers a miss to `T`'s global handlers alone (`crates/ulo/src/transport/pipeline.rs:118-152`); HTTP misses carry `NoRoute` / `MethodNotAllowed` as the `CallError` source (`crates/ulo-http/src/miss.rs:1-33`); WS unhandled event the same (`crates/ulo-ws/src/connection.rs:1523`) | The argument for globals-only, independent of any codebase: an interceptor wraps a call and a miss is not one; the target and method tiers need a target and a miss has none (memo §9, §5 nesting table). |
| S9-variant | No third `Dispatched` variant | superseded | The rebuild's HTTP routing is its own (`transports/DESIGN.md` §3.2); `Dispatched` does not exist | None. |
| F127 | The chain answers outside the interceptors, so an interceptor's stamp is absent from claimed answers | superseded | Position A kept: `dispatch` runs `claim` on the outcome of the interceptor chain (`pipeline.rs:76-83`). Stamping every answer is the pre-dispatch stage's on HTTP and gRPC, where `Next::run` answers `Ok` with whatever the inner layers produced (`transports/DESIGN.md` §3.3). RPC and WebSocket have no pre-dispatch stage (§3.4, X23) and no header slot, so the stamping case does not arise there | The probe: an interceptor stamping `x-stamped` put it on a 200 and not on a 418 the chain recovered with (F127). |
| F127-doc | F127's "documentation fix on `Interceptor`" | open | `Interceptor` carries no doc saying it never sees an error handler's answer (`crates/ulo/src/transport/mod.rs:85-91`); DESIGN §7 step 4 does not say it either. Not in the ledger | The sentence to carry, from memo §5: an inner layer never observes what its outer layer does; a stamp that must reach every answer goes in the pre-dispatch stage. |
| D3 | The error chain stays outside the interceptors | built | as F127 | Keep the three rejected alternatives with their reasons (memo §10 D3): moving the chain inside the stack trades the one-seam property for the stamping case; running it at both ends hands every interceptor an answer and takes away the typed error logging wants; widening `Interceptor`'s after-phase costs a signature change to buy what the pre-dispatch role provides. And the forcing argument: an interceptor can fail, so the chain must be above it or that failure has no claimant. **Fetched, not recalled:** Nest's lifecycle runs "8. Interceptors (post-request) · 9. Exception filters · 10. Server response", and "as soon as an uncaught exception is encountered, the rest of the lifecycle is ignored and the request skips straight to the filter" (memo §5). |
| §5-nesting | The two-position chain: outer (globals, pre-dispatch failures and misses) above pre-dispatch, inner (method, controller, global) below dispatch | built (HTTP, gRPC); superseded (RPC, WS) | HTTP: unscoped failure offered to globals only, scoped failure to the matched handler's tiers then globals (`transports/DESIGN.md` §3.3, "in the scoped sub-step the matched handler's tiers run before the global ones"); gRPC through the same `Stage` (`crates/ulo-grpc/src/pre_dispatch.rs:75`). RPC and WS route in-framework and offer a miss to globals (`recover(None, ..)`) with no outer stage | The property claimed and worth stating in any record: the inner chain always resolves to an answer, so the outer one only meets a pre-dispatch failure or a miss, and no error is offered twice (memo §5). The rebuild's scoped sub-step refines it: a scoped entry's failure gets the matched route's tiers, which the memo's diagram gave only to dispatch. |
| new-1 | Interceptors see a handler's panic as an `Err` from `Next::run` | built (beyond the memo) | `crates/ulo/src/transport/next.rs:22-39`; DESIGN §7 step 4 | The memo's world rendered a nested panic per level because `InterceptorNext::run` answered `R` (F60). The rebuild's `Result<T::Reply, BoxError>` removes that constraint, which is what made one walk possible. |

### 3.3 Scope and the roles' arms (memo §4)

| ID | What | Status | Evidence at HEAD | Value to extract |
| --- | --- | --- | --- | --- |
| F126 | Execution-scoped `ErrorHandler` or `Middleware` registered no role; refusal blamed provider list and trait | built (error-handler half); built differently (middleware half) | Error handlers have every spelling and scope: `error_handler`, `_value`, `_arc`, `_with`, `_with_in` (`crates/ulo/src/transport/enhancer.rs:127-166`), resolved per call in `claim` (`pipeline.rs:256-290`). A by-type middleware is resolved per request through its declaring module, so "a per-execution middleware is built once per request" (`crates/ulo-http/src/middleware.rs:39-40`, `pre_dispatch.rs:290-296`) | The decided fix was to *refuse* a non-singleton middleware (D2, row 63); the rebuild *accepts* it. The diagnostic lesson stands for any refusal: the old message named two conditions that both held and not the one that failed (F126). |
| F51 | Error handler cannot be request-scoped; only type names say so | built | as F126 | None. |
| F56 | Per-execution construction unreachable from the declaration site | built | `guard_with` / `guard_with_in` and kin; a closure's parameters are injection points and its scope is inferred from what it reads (`enhancer.rs:16-25`, `:60-84`); pinned at `crates/ulo/tests/enhancers.rs:520`, `:541` | The rebuild goes past ADR-0055's grammar table, whose closure cell says DI arrives "through `ctx`"; BUILD_SEQUENCE row 27's note already recorded that a sync closure cannot await a resolution. The rebuild's answer (parameters as injection points, `Auto` inferred from them) is the one to record. |
| Grammar | Three spellings told apart by grammar: bare path = type, expression = value built once, closure = constructor | superseded (spelling) / built (three lifecycles) | `#[guards(AuthGuard)]`, `#[guards(value = ..)]`, `#[guards(with = ..)]`, `#[guards(with(execution) = ..)]` (DESIGN §7). The forms are keyword-tagged rather than told apart by `{}` | The memo's point that `X{}` is what distinguishes a unit-struct value from a type name is moot under keyword tags. Rejected alternative still relevant: deleting the value form loses the shared-stateful case a rate limiter needs (memo §10). The rebuild adds a property the memo lacked: an impl-level `value` is built once per controller and shared by every handler (DESIGN §7; `enhancer.rs:51-58`). |

### 3.4 Duplication left by #300–#312 (memo §7)

| ID | What | Status | Evidence at HEAD | Value to extract |
| --- | --- | --- | --- | --- |
| F128 | Three resolvers differing by noun and one field; `Resolved<T>` unbuilt | superseded | One `dispatch<T: Transport>` for every transport (`crates/ulo/src/transport/pipeline.rs:62`); RPC, WS and gRPC call it (`crates/ulo-ws/src/connection.rs:928`, `:1555`) | The measurement rule (collapse below about six differing lines after substituting the transport noun, split above about thirty-five; AUDIT3 §4, memo §7) is a reusable test for any future per-transport copy. |
| F129 | gRPC's second interceptor chain; three `safe_render` copies | superseded | One `Next` walk (`next.rs`); one renderer per error, `CallError::from_boxed` (`crates/ulo-transport/src/error.rs:280-300`) | The E0308 finding is general Rust and still applies to `erase_build` (`enhancer.rs:294-302`): `Box::new(ClosureLeaf(\|c\| ..))` fails to infer a higher-ranked signature where a constructor bounded on it compiles (F129, AUDIT3 §2). |
| F57 | Per-call tier merge on three transports | superseded | Tiers are walked in place by iterator chaining (`pipeline.rs:185`, `:218`), no merged copy | See F122-perf. |
| F60 | Chain built four times; panic rendered three | superseded | as F129 | None beyond F129. |
| F74 | Eight per-transport entry aliases | dropped | The entry types are gone; `Decl<R>` is `pub(crate)` (`enhancer.rs:203`) | None. |

### 3.5 The vocabulary (memo §8; "one error vocabulary")

| ID | What | Status | Evidence at HEAD | Value to extract |
| --- | --- | --- | --- | --- |
| F130 | Guard `false` is `Forbidden` on a message and `Unauthorized` on a WS connect; gRPC hand-built guard messages | built | Both WS phases run `ulo::dispatch` (`crates/ulo-ws/src/connection.rs:928` connect, `:1555` message), so a refusal is `GuardRejected` in both and renders through `from_boxed` as `Forbidden` with the constant message `"forbidden"` (`crates/ulo-transport/src/error.rs:251`, `:293-295`). No test asserts the two phases render the same kind | Test idea (build handoff, PR 8): one guard refusing a connect and the same guard refusing a message raise the same event and render the same kind. The cause analysis is the durable part: a second vocabulary beside `ErrorKind` picked the kind by accident (F136). |
| F131 | `GuardRejection::with_reason`, `PanicRecovered::during`, `PipelineSegment::Other` had no producer; a guard could not say why | superseded | `GuardRejected { guard: &'static str }` with a `pub(crate)` constructor (`crates/ulo/src/error/mod.rs:550-558`); `PanicRecovered::new` `pub(crate)` (`:578-582`); `DispatchStage` has five variants, all produced (`:595-603`). A guard with a reason returns its own `Err` (`transport/mod.rs:73-80`) | Ledger-worthy rule from the memo: a public field documenting an input the trait cannot supply is a defect class (F131). The rebuild's `GuardRejected` passes it: no field without a producer. |
| F132 | Four pieces of stale production text | dropped | Every cited file is gone | None. |
| F136 | Each transport's error a second vocabulary beside `ErrorKind` | built | One `CallError { kind, message, details, source }` rendered by every transport, `Classify` for domain errors (`transports/DESIGN.md` §2.4); no `HttpError` or `WsError` type exists (`grep` over `crates/`); `RpcError` survives as the RPC *client's* error only (`crates/ulo-rpc/src/client.rs:1291`) | The decided shape (`Status { kind, message }` / `Custom { status, message, source }` / `AppError` per transport) is superseded by one struct. What carries over is the argument that `Custom { status: 429 }` rendered the status and lost the kind (F136); `CallError` has no raw-status escape, which closes it. |
| doc-1 | Core DESIGN §7 example names `HttpError` and `RpcError` as handler error types | open | `DESIGN.md` §7 (`get(..) -> Result<Json<User>, HttpError>`, `get_rpc(..) -> Result<User, RpcError>`); neither is a server-side type at HEAD (see F136). Not in the ledger | A one-line doc correction; it reintroduces in the design's own example the second vocabulary F136 removed. |

### 3.6 Globals and tokens ("also settled")

| ID | What | Status | Evidence at HEAD | Value to extract |
| --- | --- | --- | --- | --- |
| F133 | `APP_MIDDLEWARE` exported and read by nothing | superseded | Middleware is declared through `PreDispatch::apply::<M>()` and checked by `wire()` as a dependency (`pre_dispatch.rs:143-156`, `:298-300`); no `APP_*` const exists | The failure mode generalises: a registration read by nothing is indistinguishable from a middleware that passed the request through (F133). The rebuild's guard against it is that every entry declares its dependencies to `wire()`. |
| F134 | Token route for a global served HTTP alone | built | Role keys `AnyGuard<T>`, `AnyInterceptor<T>`, `AnyErrorHandler<T>` for every `T: Transport` (`transport/mod.rs:149-151`, `:176`); `m.contribute::<AnyGuard<Http>>().provide::<X>(..)` resolves from DI (DESIGN §7) | None. |
| F135 | A global error handler could not resolve from DI | built | as F134; pinned by `crates/ulo/tests/enhancers.rs:418` (a global error handler's panic offered to the one contributed before it) | None. |
| Globals-matrix | One registration surface per transport per role, token or value | built | as F134 | The count that motivated it: 2 of 16 transport-by-role cells could resolve from DI (F134). |
| F50 / PIPE-6 | `#[use_guards(APP_GUARD)]` documented and did not compile; `Token<T>` consts in the attribute | dropped | String tokens and `Token<T>` are gone; keys are `TypeId`s (DESIGN §0.5) | The memo's sub-point stands as a design rule: naming a global at a controller is confused on its own terms (memo §10). |

### 3.7 The API pass (F137)

| ID | What | Status | Evidence at HEAD | Value to extract |
| --- | --- | --- | --- | --- |
| F137 | Three role traits each spelled their transport differently; `InterceptorNext` public | built (core of it) | `Guard<T>`, `Interceptor<T>`, `ErrorHandler<T>` over one `T: Transport` (`transport/mod.rs:81-105`) | None. |
| API-1 | `Transport` public **and sealed** | superseded | Public, unsealed, implemented in transport crates (`transport/mod.rs:23-33`); `Role` is the sealed trait (`:153-176`) | The memo's reason for sealing ("a user's own marker would get no adapter and no dispatch") does not hold once `ulo::dispatch` is public and transports live in their own crates. |
| API-2 | Concrete `Next<T>`, the continuation's trait private | built | `pub struct Next<'a, T>` with `run(self)` (`crates/ulo/src/transport/next.rs:9-41`) | The rebuild names two `Next`s (`ulo::Next`, `ulo_http::middleware::Next`) and documents the import collision rather than aliasing (`crates/ulo-http/src/middleware.rs:1-6`). |
| API-3 | Context first everywhere; `handle_error` renamed `catch` | open | `ErrorHandler::handle(&self, err: BoxError, cx: &T::Cx)` (`transport/mod.rs:103-105`): error first, named `handle`. DESIGN §0.3 names a trait after its predominant method, which yields `handle`, and is silent on argument order | See §4 Q5. |
| API-4 | One result alias | superseded | No aliases; every role answers `Result<T::Reply, BoxError>` | None. |
| API-5 | `HasHeaders` capability trait for transport-generic stamping | dropped | No `HasHeaders` in `crates/` (`grep`); `T::Reply` is opaque to a generic interceptor | Rejected alternative with a reusable argument: Nest's runtime narrowing (`getType()` / `switchToHttp()`) has an `else` branch that returns a wrong answer with nothing to report it, where a bound refuses to compile (memo §10). The case it served (stamping) moved to the pre-dispatch stage on the two transports with headers. |
| API-rule | "A trait carries what is true of everything that implements it; anything narrower goes on a narrower trait" | built (as practice) | `Transport::Reply` is per transport and no answer type sits on `Execution`; `HttpCarried` restricts `PreDispatch<T>` to the two transports carried on `http::Request` (`transports/DESIGN.md` §3.4, X23) | Keep the rule verbatim with its two worked rejections: an `Answer` on `ExecutionContext` failed because `StandaloneContext` has no answer; a header slot on `Interceptor` failed because two wires have none (memo §10). |

### 3.8 Middleware as one role per position (memo §6; D2)

| ID | What | Status | Evidence at HEAD | Value to extract |
| --- | --- | --- | --- | --- |
| §6-synthesis | On HTTP one role existed twice (`Middleware` and `Interceptor<HttpContext>`) with two contracts | superseded | Module anchor gone; `Middleware` exists only at pre-dispatch (`transports/DESIGN.md` §3.3) | The comparison table (memo §6) is the argument for "one role per position". The sentence that explains why: module middleware sat *outside* the error chain, in the global role's position merely scoped per route, so its failures missed `#[catch]` because of where it sat. |
| D2 | `Middleware` narrows to pre-dispatch **on all four transports**; module anchor retired; path scoping over the framework table; tower at the anchor | superseded (built differently, with a loss) | HTTP and gRPC only: `PreDispatch<T: HttpCarried>` (`pre_dispatch.rs:79`, `crates/ulo-grpc/src/pre_dispatch.rs:27`); "WebSocket and RPC have no pre-dispatch stage: guards and interceptors cover them" (`transports/DESIGN.md` §3.4). Module anchor retired, path scoping by route pattern, tower at the stage: built | The memo's argument for four transports, which the rebuild did not adopt: "before the target is known" is well defined on RPC (pattern match) and WebSocket (event match), both in-framework, and middleware was HTTP's only because HTTP was the only transport when it arrived (memo §6). What RPC and WS lose without it: nothing that sees a call before its target is chosen, so a cross-cutting auth or rate limit there must be a global guard, which runs after routing and never on a miss. |
| D2-change | The behavioural change to state: code moved from module middleware into an interceptor moves *inside* the chain | open | The switch's release-notes table names "per-module middleware by route" as covered (`SWITCH_INVENTORY.md:43`, `:133`) and states no behavioural change. Not in the ledger | A migration line for users of `configure_middleware`: a module middleware's `Err(HttpError)` rendered directly; its replacement (scoped entry or interceptor) reaches the error handlers. |
| F118 | Middleware could be pre-routing or path-scoped, not both; auth on `/admin` must answer for unrouted paths under `/admin` | **open** (built differently, with a loss) | Scoped entries run only after a route matched (`pre_dispatch.rs:4-9`, `:513-542`); an unscoped entry's `exclude` is negative only (`:257-266`). `crates/ulo-http/tests/middleware.rs:225` asserts `/admin/nowhere` answers 404 with the scoped entry not run. Not in the ledger | **The enumeration argument (F118), standalone:** an auth check scoped to `/admin/*` that skips misses answers an anonymous caller 404 for a path no route claims and 401 for one a route claims, so the set of admin routes is readable without credentials. Express's `app.use("/admin", mw)` runs on the request path, misses included; the rebuild's unscoped-plus-own-prefix-check is the only spelling that closes it. See §4 Q1. |
| F119 | `for_route` took a registered pattern; a concrete path matched nothing, silently | built | `prepare` refuses a scope covering no route, with its location (`pre_dispatch.rs:465-490`); pinned at `middleware.rs:390`, `:420`, `:428` | None. |
| F73 | `MiddlewareConsumer` an HTTP type under `di/` | superseded | No `MiddlewareConsumer`; pre-dispatch metadata lives in `ulo-http` | The module rule's test (a type belongs to a transport iff meaningless on the other three) and the reason the old grep missed it: it named *routes*, not `Http` (F73). |
| Tower | Tower mounts at pre-dispatch; scoped "this layer on `/admin/*`" | built | `layer` / `layer_for` (`pre_dispatch.rs:176-220`; `transports/DESIGN.md` §3.4) | The memo's reason, still the right one: a tower `Service` is `http::Request -> http::Response`, the stage's signature; post-dispatch no request is left to hand it (memo §6). |
| S13 | gRPC's pre-dispatch anchored above tonic's router, covering `add_service`d services | superseded | gRPC dispatches by `:path` over tonic's `server::Grpc` without tonic's server (`transports/DESIGN.md` §1, §6.2); health and reflection run after the unscoped entries only | None for this subject. |
| Name | "Middleware" kept as the name | built | `ulo_http::middleware::Middleware` | Express means pre-routing by "middleware", so narrowing the role makes the borrowed word accurate (memo §6). |

### 3.9 Guards' answer (D4) and the records (D5)

| ID | What | Status | Evidence at HEAD | Value to extract |
| --- | --- | --- | --- | --- |
| D4 | A guard answers `Result<(), E>`, `E: Into<T::Error>`, framework attaching the guard's index | superseded | `Result<bool, BoxError>`; `Ok(false)` becomes `GuardRejected { guard }` named by type (`pipeline.rs:192-198`); `Err(CallError::unauthorized(..))` for 401 (`transport/mod.rs:73-80`). This is ADR-0055's rejected alternative | ADR-0055's rejection reason ("two refusal paths that render differently, the class F130 already is") is answered in the rebuild by giving each path a meaning and routing both through the error handlers; F130's defect was one decision rendering two ways by accident. The record has not caught up (OP-3). |
| D5 | Two ADRs: the pipeline's order (0055), the pre-dispatch role (0056) | open | Both `Status: proposed` in `docs/adr/`, indexed (`docs/adr/README.md:70-71`), describing the old API (`#[use_guards]`, `HttpError`, `Status { kind, message }`, `HasHeaders`, `use_global_middleware`, four-transport pre-dispatch, refusing a non-singleton middleware). `SWITCH_INVENTORY.md:40` and `:297` cite them as evidence for the rebuild | See §4 Q2. Each ADR's *Roads not taken* section is the part worth carrying into whatever replaces it; the decisions themselves are mostly superseded. |
| S6 | ADR-0052 amendments: strike "runs the global interceptors"; F125 constraint on `dispatch` | superseded | ADR-0052 is not in `docs/adr/` on this branch (`ls docs/adr`); both behaviours are built (S9-gate, F125) | None. |

### 3.10 Panic text and adjacent findings named by the handoff

| ID | What | Status | Evidence at HEAD | Value to extract |
| --- | --- | --- | --- | --- |
| F242 / REN-1 / S5 | A recovered panic's payload reached the caller on all four transports | built | `from_boxed` renders `PanicRecovered` as `Internal` with a generic message (`crates/ulo-transport/src/error.rs:290-292`); the payload is kept, redacted, on `PanicRecovered::message` (`crates/ulo/src/error/mod.rs:572-576`); middleware test asserts withheld text (`middleware.rs:265`) | S5's rule, worth stating anywhere a framework event is rendered: `Display` is the diagnostic, the public message is what the caller is told (BUILD_SEQUENCE S5). |
| F84 | Extraction failure answered outside the chain | superseded | `ExtractError` implements `Classify` and reaches the error handlers as a `CallError` with it as source (`transports/DESIGN.md` §2.2); `from_boxed` maps it (`error.rs:283-289`) | Owned by the extraction subject. The sweep table in F84 (every site answering without the chain, each routed or recorded with its reason) is a reusable audit method. |
| F88 | On RPC and WS, rendering inside an error handler changes nothing | UNVERIFIED | Owned by the render subject; not checked here | Would need the RPC and WS reply shapes at HEAD read against an error handler answering `Ok(rendered)`. |

### 3.11 Build rows (BUILD_SEQUENCE) and the handoff's verification list

| ID | What | Status | Evidence at HEAD | Value to extract |
| --- | --- | --- | --- | --- |
| Row 26 PIPE-ADR | The two ADRs, proposed (#364) | built (on master; files present) | `docs/adr/0055*`, `0056*` | Row note: the handoff put the non-singleton `Middleware` refusal in 0056 while row 63 built it and flipped only 0055. Moot now (see F126). |
| Row 27 PIPE-1 | One ordered vector; closure form (#365) | superseded | Merged at `3cc3651`, replaced by the rebuild (§3.1) | The row note's correction to ADR-0055 (a sync closure cannot reach DI "through `ctx`") is the one fact not elsewhere. |
| Row 28 PIPE-2 | Guards lazily, interceptors after (#366) | superseded | as row 27; rebuilt (§3.1) | Row note: F248's guard half (a panic while *building* an execution-scoped guard escaped recovery) stayed open on master. The rebuild catches it: a construction panic arrives as `LookupError::Construct { reason: Panicked }` (`pipeline.rs:27-31`, DESIGN §7 step 4). |
| Row 29 PIPE-3 | Stale texts (#367) | dropped | Texts gone with the code | Row note: the article bug ("A errorhandler") was fixed by carrying a display name beside the identifier rather than deriving it; a reusable pattern. |
| Row 30 PIPE-4 | `Resolved<T>`, pre-merged tiers, `closure_leaf`, one `safe_render` (#368) | superseded | §3.4 | None. |
| Row 33 / PIPE-6 | Absorbed by DI-2 | dropped | §3.6 F50 | None. |
| Row 55 PIPE-5 | One global surface per transport per role | built | §3.6 | None. |
| Row 56 PIPE-7 | One error vocabulary, three-field `Custom` | superseded | §3.5 F136 | The `source` slot's reason (a hand-named status keeping the domain error a `#[catch]` matches on) is met by `CallError::source` and `source_as` (`transports/DESIGN.md` §2.4). |
| Row 57 PIPE-8 | A guard's refusal one event on four transports and both WS phases | built | §3.5 F130 | Test idea open: see F130. |
| Row 60 PIPE-9 | The API pass | superseded | §3.7 | None beyond §3.7. |
| Row 62 PIPE-10 | Guard answers `Result<(), E>` | superseded | §3.9 D4 | None beyond D4. |
| Row 63 PIPE-11 | Error handlers' `Factory` arm; refuse non-singleton middleware | built (first half); superseded (second half) | §3.3 F126 | None. |
| Row 101 PIPE-12 | Pre-dispatch on four transports, path scoping, tower, module anchor retired | superseded | §3.8 D2 | None beyond D2 and F118. |
| Row 81 HTTP-3 (F125 constraint) | A global chain's failure reaches the global handlers | built | §3.2 F125 | None. |
| V1 (PR 1) | Mixed spellings run in written order; permuted, in permuted order; tier pair; error handlers; closure per execution, value once | built (partly) | `crates/ulo/tests/enhancers.rs:90`, `:226`, `:520`, `:593` | The permutation falsification is absent (see F121). |
| V2 (PR 2) | Execution-scoped guard and interceptor below a refusing guard record no construction | built | `enhancers.rs:174` | None. |
| V5 (PR 5) | A token-registered global resolves with its dependencies on each transport, each role; falsify by removing the provider | open (test) | Core tests use a test transport `TP` (`enhancers.rs:55-84`); no per-transport, per-role matrix test found | Test idea: 4 transports by 3 roles, each global contributed by type with one dependency, plus the removed-provider refusal at `wire()`. |
| V8 (PR 8) | Connect guard and message guard raise the same event, same kind | open (test) | See F130 | As stated. |
| V11 (PR 11) | Execution-scoped error handler built per execution; non-singleton middleware refused | built (first half); superseded (second half) | §3.3; `crates/ulo/tests/enhancer_roles.rs:319-320` declares scoped error-handler closures | A test asserting an execution-scoped error handler is built once per failing call was not found; `crates/ulo/tests/enhancer_roles.rs` exercises declared scopes on closures. |
| V12 (PR 12) | Module middleware's domain error, moved to an interceptor, claimable; pre-dispatch failure reaches globals on every transport; pre-dispatch sees a response a `#[catch]` handler produced | open (third clause) | Second clause built on HTTP (`middleware.rs:269`); third clause unpinned (OP-4) | **The audit's own unrun probe.** Write it as: an outer unscoped CORS entry, an inner entry returning `Err(CallError::unauthorized(..))`, and separately a handler error claimed by a global error handler; assert the outer entry's headers on both answers, on HTTP and gRPC. |

### 3.12 Process and method (both handoffs)

| ID | What | Status | Evidence at HEAD | Value to extract |
| --- | --- | --- | --- | --- |
| P-method | Probe, then falsify; fetch specs, never recall; confidence marking per claim (Probed / Read / Measured / Fetched / Unverified) | built (as practice) | The audit handoff's method section and the memo's confidence key | Two recorded catches: a Nest claim written from memory that the fetch corrected (memo §0, §5), and "one wrong citation during this audit's own verification pass" (build handoff). The recording-guard probe shape (append to a static, assert the sequence) is what `crates/ulo/tests/enhancers.rs` uses. |
| P-probes | Four probe files written under `integration-tests/tests/integration/` and removed | dropped | Against the old macro and DI path only; none applies to the rebuilt code | Their results are carried in F121, F122, F123–F127 above. |
| P-spec | The pipeline is category (3); only the HTTP error body crosses a specified wire | built | RFC 9457 `application/problem+json`, and RFC 9110's `WWW-Authenticate` on 401 (`transports/DESIGN.md` §2.4 renderings table) | The check itself, as a method: for each output the pipeline writes, ask whether a standard specifies its wire. The rebuild found a second one the memo did not (RFC 9110 requires the challenge on 401). |
| P-build | Divergence gate, the Law, per-PR checklist, bookkeeping, merge mode | dropped | The old build sequence is paused; nothing executes it | None for the designer. |
| P-paths | F56, F57, F60 path translations after #306 | dropped | Paths gone | None. |

## 4. Worth discussing with the designer

**Q1 (OP-1, F118). Should a route-scoped pre-dispatch entry run on misses under its scope?**
Today `apply_for::<Auth>(["/admin/*"])` runs only after a route matched, so an unauthenticated
caller can enumerate `/admin/*` by status (404 vs 401), and the suite pins that as intended
(`crates/ulo-http/tests/middleware.rs:225`). Options: (a) a request-path scope on unscoped entries
(`apply::<Auth>().only(["/admin/*"])`, matched against the path as the stage receives it,
misses included), the positive twin of the existing unscoped `exclude`; (b) document that
path-guarding auth must be unscoped with its own prefix check. Recommendation: (a). The unscoped
`exclude` already matches request paths (`pre_dispatch.rs:65-68`), so the matcher exists, and
the leak is the one F118 was filed for.

**Q2 (OP-2, D5). What happens to ADR-0055 and ADR-0056?** Both are `proposed` and indexed, and
the rebuild contradicts them in eight places (§2, item 2). Options: supersede both with one record
of the rebuild's pipeline (DESIGN §7 plus transports §3.3) that keeps their *Roads not taken*;
amend them in place; or withdraw them. Recommendation: one superseding record. The rejected
alternatives (§3.2 D3, §3.7 API-5, §3.1 D1, §3.3 Grammar) are the content nothing else in the tree
carries, and keeping them in a `proposed` record that describes the wrong API makes them unreadable.

**Q3 (OP-3, D4). Record why `Result<bool, BoxError>` was chosen over the decided `Result<(), E>`.**
The rebuild's split (403 for `Ok(false)`, the error's own kind for `Err`) is defensible and is the
alternative ADR-0055 rejected. Recommendation: state in the superseding record that both paths reach
the error handlers, so the F130 class (one decision, two renderings, unclaimable) cannot recur, and
that `Ok(false)` exists so the common refusal needs no error type.

**Q4 (D2). Pre-dispatch on RPC and WebSocket.** The memo argued the moment "before the target is
known" is well defined on both; the rebuild says guards and interceptors cover them. The cases that
differ: a check that must run on a miss (an unhandled RPC pattern or WS event), and one that must
run before routing cost is paid. Recommendation: keep the rebuild's position and write the reason
into the record, since both transports route in-framework and offer a miss to the global error
handlers already; reopen only on a concrete miss-path use case.

**Q5 (OP-5, API-3). `ErrorHandler::handle(err, cx)` vs the API pass's `catch(cx, err)`.** Guard and
Interceptor take `cx` first; the error handler takes the error first. Recommendation: a judgement
call with low stakes. If the order changes, it changes before any external user exists; DESIGN §0.3
already settles the name `handle`.

**Q6 (doc-1). DESIGN §7's example names `HttpError` and `RpcError`.** Recommendation: replace with
`CallError` or a `Classify` domain error, as transports §3.1 does.

**Q7 (F127-doc, D2-change). Two production texts owed.** `Interceptor`'s doc should say it never
sees an error handler's answer and where a stamp on every answer goes; the switch's release notes
should state that per-module middleware is replaced by scoped entries and interceptors that sit
inside the error chain. Recommendation: both, as one-line edits.

## 5. Not verifiable here

- **OP-4 / V12's third clause:** whether an outer pre-dispatch entry's headers reach the answer an
  inner entry's `Err` became, and an error handler's claimed answer, on HTTP and gRPC. Reading says
  yes (`transports/DESIGN.md` §3.3; `Next::run` answers `Ok`); a run of the test in §3.11 decides it.
- **F122-perf:** the per-call `Vec` in `build_interceptors` and the per-failure `collect` in `claim`
  (`crates/ulo/src/transport/pipeline.rs:213-222`, `:273-276`) are unmeasured; a benchmark decides
  whether they matter.
- **F88:** needs the RPC and WS reply shapes at HEAD (render subject).
- **V5 and V8:** the test ideas are written out above; whether the per-transport suites
  (`ulo-*-conformance`) already cover them indirectly was not established by `grep` and needs a
  read of each conformance crate's cases.
