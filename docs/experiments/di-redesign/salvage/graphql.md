# Salvage: the GraphQL audit

## 1. Header

**Subject.** The GraphQL audit of 2026-09-22, run against master `654c7183`: `ulo-graphql-async-graphql`
and `ulo-graphql-juniper` read against GraphQL-over-HTTP and graphql-transport-ws, then a second
half on what ulo should own (engine, resolver seam, bindings). It filed F182-F195, settled D1-D8 and
planned ten PRs in three phases (BUILD_SEQUENCE rows 22, 23, 123-130).

**Source files read in full** (`docs/experiments/di-redesign/salvage/source/graphql/`):

| File | Words |
| --- | --- |
| `GRAPHQL_AUDIT.md` | 4566 |
| `GRAPHQL_BUILD_HANDOFF.md` | 2161 |
| `GRAPHQL_AUDIT_HANDOFF.md` | 409 |
| `artifacts/resolver-seam-probes/probe_resolver_seam.rs` | 396 |
| `artifacts/resolver-seam-probes/probe_resolver_seam2.rs` | 465 |
| `artifacts/resolver-seam-probes/probe_resolver_race.rs` | 283 |
| `artifacts/resolver-seam-probes/probe_error_path.rs` | 419 |
| `artifacts/resolver-seam-probes/probe_sub_error.rs` | 301 |
| `artifacts/resolver-seam-probes/probe_isolate.rs` | 205 |
| total | 9205 |

Also read: `FRAMEWORK_GAPS.md` F182-F195, F198, F456, F464, F490; `source/BUILD_SEQUENCE.md` rows 22,
23, 46, 89, 123-130, S10 and the row-22/23/33 notes; `source/README.md` couplings.

**Date** 2026-11-01. **Tip** `331c35b9` (`experiment/di-redesign`).

**HEAD surface checked.** Five crates: `crates/ulo-graphql` (the `Engine` SPI, `GqlRequest`,
`GqlResponse`, `Outcome`), `crates/ulo-graphql-async-graphql`, `crates/ulo-graphql-juniper`,
`crates/ulo-graphql-http` (endpoint, status rules, playground, `GraphqlModule`), `crates/ulo-graphql-ws`
(the graphql-transport-ws gateway); their tests; `transports/DESIGN.md` §7 (lines 1231-1283) and §13
decision 29 (line 1597); `transports/REVIEW_2B.md` R36/R37; `transports/DIVERGENCES_2B.md` U26 and the
`Accept` divergence (line 530-542); `divergences/race2b-followups.md`. The integrations `DESIGN.md`
carries no GraphQL section; GraphQL lives in transports DESIGN §7.

**Spec re-read.** GraphQL-over-HTTP was fetched again on 2026-11-01 from
`raw.githubusercontent.com/graphql/graphql-over-http/main/spec/GraphQLOverHTTP.md` (status line
"Current Working Draft"), through a summarising reader; the sentences quoted below as "draft
2026-11-01" are what that reader returned as verbatim and should be confirmed against the raw file
before a decision rests on them. Three of them contradict what the rebuild's DESIGN §7 and REVIEW R36
state the specification says (§4 Q1-Q3).

## 2. Summary

| Status | Count |
| --- | --- |
| built | 44 |
| superseded | 15 |
| open | 10 |
| dropped | 14 |
| UNVERIFIED | 1 |
| total | 84 |

"built" includes items built differently from the plan where the behaviour the item asked for exists;
where something was lost on the way the row says so.

**The five most valuable open items**

1. **N1: request errors answer `400` where the specification gives `422` for validation, variable
   coercion, an undeterminable operation and a malformed request.** HEAD's `Outcome` has one
   `RequestError`, so the HTTP binding cannot write `422` at all (`crates/ulo-graphql/src/lib.rs:118-125`,
   `crates/ulo-graphql-http/src/controller.rs:149-155`). D5's taxonomy, which the audit made the one
   SPI obligation that "cannot be deferred", was built one level coarser. Not in the ledger.
2. **N2: the legacy `application/json` path answers `200` for every response.** The audit's plan
   (handoff PR 5 verification) and the draft both keep the status codes for a legacy client and
   replace only `Content-Type` on a `2xx` (`controller.rs:145-155`). Not in the ledger.
3. **GQL-7 / §7: the graphql-sse binding is not built and not in DESIGN.** It is the falsifier the
   audit named for the per-binding rule; HEAD's `Outcome` happens to support it (§3.5).
4. **D3/D4/F195: the resolver seam (Path B) is not built, and graphql-transport-ws has no
   per-operation authorization point** (DESIGN §13 decision 29). The six probes prove the seam
   feasible on the old API; the mechanism they prove is the one HEAD's adapters already use for the
   context.
5. **Two schemas through qualifiers have no test.** DESIGN §7's "Two engines" paragraph
   (`dyn Engine @ Admin`, `GraphqlConfig::engine::<E>()`) is unexercised: no test or example calls
   `.engine::<_>()`, and `the_same_schema_at_two_paths_mounts_both` uses one engine
   (`crates/ulo-graphql-http/tests/mount.rs:277-296`). This is the case F191 found broken.

## 3. Items

### 3.1 Findings F182-F195 (the HTTP surface)

| ID | What | Status | Evidence at HEAD | Value to extract |
| --- | --- | --- | --- | --- |
| F182 | every request error answered `200` | built, with a loss (N1) | `Media::status` answers `400` for `RequestError` under `application/graphql-response+json`, `500` for `Failed` (`ulo-graphql-http/src/controller.rs:149-155`); test table `mount.rs:136-161` | the 400/422 split is lost; see N1 |
| F183 | no `application/graphql-response+json`, `Accept` unread | built, with a loss (N3) | `Media::negotiate` reads every `Accept` header with RFC 9110 qualities and most-specific-range matching (`controller.rs:128-217`); twelve-row test `mount.rs:140-153` | no `406`; see N3 |
| F184 | request `Content-Type` never read | built | `is_json` gate before the body is read, refusal `ExtractError::UnsupportedMediaType` rendered 415 (`controller.rs:103-105`, `:220-226`); a POST with no `Content-Type` is 415 too (`mount.rs:204-207`) | the draft allows any `4xx` for a missing `Content-Type`; 415 is a conforming choice |
| F185 | playground served `text/plain` | built | `text/html; charset=utf-8` (`ulo-graphql-http/src/playground.rs:22-23`); was closed on master by #360 | none |
| F185 residual | `GET /graphql?query=…` answered the playground | built | the page needs `Accept` naming `text/html` with `q>0` and no `query` (`playground.rs:12-14`, `controller.rs:75-82`); `*/*` is not a browser; a `query` executes (`mount.rs:240-243`) | the `*/*`-is-not-a-browser rule is a decision worth keeping: `curl` sends `*/*` |
| F186 | non-map `variables` discarded, wrong error given | built, with a loss (N1) | `GqlRequest.variables: Option<Map<..>>` makes a string fail deserialization; POST answers request error "the body is not a GraphQL request" (`controller.rs:108-111`); GET `json_object` refuses non-objects (`:230-234`) | refused, no longer discarded; the status is 400 where the draft gives 422 for a request that is not well-formed |
| F194 | `extensions` dropped | built (async-graphql), dropped (juniper) | `GqlRequest.extensions` (`ulo-graphql/src/lib.rs:50-51`); async-graphql copies each into `request.extensions`, a non-GraphQL value a request error (`ulo-graphql-async-graphql/src/lib.rs:144-155`); juniper drops them, documented (`ulo-graphql-juniper/src/lib.rs:228`) | juniper 0.16's `GraphQLRequest` has no extensions slot; a persisted-query extension works on one engine only, which no document outside the crate says |
| F195 | endpoints take no guards/interceptors/metadata; feature lists claimed otherwise | superseded | the endpoint is an ordinary HTTP controller mounted `.at(path)`, so HTTP guards, interceptors and pre-dispatch entries apply to it (`ulo-graphql-http/src/lib.rs:1-4`, DESIGN §7 "HTTP") | per-field enhancers (D4) are not built; see D4 |

### 3.2 Findings F187-F193 (subscriptions, DI, juniper)

| ID | What | Status | Evidence at HEAD | Value to extract |
| --- | --- | --- | --- | --- |
| F187 | none of the five close codes | built | `ulo-graphql-ws/src/gateway.rs`: 4400 `:254-256`, 4401 `:321-322,338`, 4408 `:218-240`, 4429 `:273-277`; 4406 at connect `:215-217`; tests `protocol.rs:320-421`, `init_refused.rs:261-325` | the close reasons are pinned beside their codes (`gateway.rs:33-38`, CORE_TEST_PLAN C9) |
| F188 | duplicate id runs two streams, orphans the first abort handle | built | `Admission::Duplicate` → 4409 (`gateway.rs:323-324,339-343`); a sequence number tells a finished operation from a reused id (`:99-124`); test `protocol.rs:355`, `:296` | the reason string is shortened to fit 123 bytes when the id is long (`:340-341`) |
| F189 | `error` frame declared and never sent | built | `RequestError`/`Failed` written as `error` with no `complete` (`gateway.rs:375-380`); test `protocol.rs:423` | none |
| F190 gateway half | equality test on a list-valued header | built | the gateway reads the subprotocol the transport selected (`gateway.rs:215`) | none |
| F190 adapter half (= F198) | adapters echo the raw header | built | `ulo-ws` splits every `Sec-WebSocket-Protocol` line on commas, trims, and picks the first of the gateway's declared subprotocols the client offered (`crates/ulo-ws/src/connection.rs:787-795`) | none; BUILD_SEQUENCE row 89 (WS-13) was this |
| F191 | one fixed `"GraphQLService"` token; two modules picked by hash order | superseded | no token exists: the engine is bound as `dyn Engine` or `dyn Engine @ Q`, read by `Dep<dyn Engine, Q>` (`ulo-graphql-http/src/controller.rs:32-48`, DESIGN §7 "Two engines") | the probe result is the regression test owed: `imports: [alpha(), beta()]` panicked `Failed to downcast 'GraphQLService' to AlphaSvc`, `[beta(), alpha()]` answered the other module's service silently. No HEAD test binds two engines (item 5 of §2) |
| F192 | juniper's `Subscription` parameter required and unused | built | `Juniper<Q, M, Sub>::subscribe` over `resolve_into_stream`, served by the shared gateway (`ulo-graphql-juniper/src/lib.rs:100-109,148-217`); test `ulo-graphql-juniper/tests/engine.rs:227` | none |
| F193 | juniper header comment contradicts the coverage ledger | built | closed on master by #361; HEAD's juniper crate header carries no such claim (`ulo-graphql-juniper/src/lib.rs:1-12`) | none |

### 3.3 Memo observations (§1-§6)

| ID | What | Status | Evidence at HEAD | Value to extract |
| --- | --- | --- | --- | --- |
| A1.disc | `async_graphql::Response.data` has no `skip_serializing_if`, so `data:null` cannot tell a request error; `is_ok()` false for field errors too | built | `response_of` classifies `data == Null` with every error path-less as a request error (`ulo-graphql-async-graphql/src/lib.rs:200-218`); `missing_variables` runs CoerceVariableValues for absent non-null variables before the schema (`:159-192`, F456); test `engine.rs:226-271` | the wrong-type variable case stays an executed result on async-graphql and a request error on juniper (F464, open, ledger holds it, upstream filing owed) |
| A1.405 | `DELETE` answered `405` with `Allow` by the adapter | built | router 405 (transports DESIGN §3.2); a mutation over GET is a `MethodNotAllowed` with `Allow: GET, HEAD, POST, OPTIONS` offered to the error handlers (`controller.rs:249-256`); test `mount.rs:165` | none |
| A3.env | an invalid message answered ulo's canonical envelope on a protocol wire | built | 4400 close, no envelope (`gateway.rs:254-256`) | none |
| A3.lib | async-graphql's own `http::WebSocket` is wrong on three of five codes | dropped (no longer a candidate) | HEAD hand-writes the gateway (D2) | the rejected-alternative argument, kept in full: async-graphql 7.2.1 answers `3008` for the init timeout, `1002` for an invalid message, `1011` for a subscribe before ack (`src/http/websocket.rs:313,329,378`), conforming on `4429` only (`:359`), and sends no `error` frame for a request error. Delegating trades a conforming gateway for one that fails three MUST closes |
| A5.dihash | `resolve_from_imported_modules` returns the first match over an `FxHashSet` | UNVERIFIED (out of subject) | the DI was rebuilt; whether an ambiguous import is refused at HEAD is the DI salvage's to check | none here beyond F191 |
| A5.test | `two_paths_mount_two_modules` asserted only that `create` succeeded | built | `the_same_schema_at_two_paths_mounts_both` posts to each path and counts engine calls (`mount.rs:277-296`) | it still uses one engine; two schemas are untested (§2 item 5) |
| A6.copy | the two crates' controllers were one file copied | built | one endpoint and one gateway serve both engines over the `Engine` SPI (`ulo-graphql-http`, `ulo-graphql-ws`) | none |
| A6.lib | each crate reimplemented what its library ships | built | async-graphql: its `Request`, `Variables`, `parsed_query`, `GraphiQLSource`, `ServerError` serialization (`lib.rs:25-27,136-157,220-226`); juniper: `juniper::http::GraphQLRequest`/`GraphQLResponse`, `resolve_into_stream`, `graphiql_source` (`ulo-graphql-juniper/src/lib.rs:21,181,117`) | the WS state machine stays hand-written by decision (D2) |
| A6.exec | `Executor::execute_stream` returns `BoxStream<'static>`, so the gateway's spawn and channel were never needed | superseded, claim version-dependent | the lock pins async-graphql `7.0.17` (`Cargo.lock`), where the inherent `Schema::execute_stream(&self)` returns `impl Stream + Send + Unpin` (`async-graphql-7.0.17/src/schema.rs:645-648`); the audit read 7.2.1, where it returns `BoxStream<'static, Response>` (`async-graphql-7.2.1/src/schema.rs:658`). HEAD calls `execute_stream_with_session_data`, which clones the schema and is `'static` in both, and spawns no task (`ulo-graphql-async-graphql/src/lib.rs:83-85`) | HEAD's comment "`execute_stream` borrows the schema for its stream" is true for the locked 7.0.17 and false from 7.2; a dependency bump falsifies the comment, not the code |

### 3.4 The second half: §7-§8

| ID | What | Status | Evidence at HEAD | Value to extract |
| --- | --- | --- | --- | --- |
| A7.rule | conformance is per binding; no status rule in a shared GraphQL layer | built | `ulo-graphql` carries `Outcome` and no status; the status table is in `ulo-graphql-http` alone (`ulo-graphql/src/lib.rs:114-125`, `controller.rs:145-155`); the gateway maps the same `Outcome` to frames (`gateway.rs:375-380`) | the deciding citation, kept: graphql-sse's distinct-connections mode says validation errors "MUST report errors through an accepted SSE connection by emitting `next` events … responding with a `400` (Bad Request) will cause the user agent to fail the connection" (graphql-sse PROTOCOL.md), against GraphQL-over-HTTP's 4xx for the same error |
| A7.sse | graphql-sse binding, both modes (GQL-7, row 127) | open | no crate, no DESIGN section; the workspace holds five GraphQL crates, none SSE | §7's falsifier test, written to stand alone: one engine result for a document that fails validation must render `200` plus a `next` event carrying the errors on graphql-sse and the request-error status on GraphQL-over-HTTP. HEAD's `Outcome::RequestError` is sufficient input for both. Not in the ledger |
| A7.multipart | incremental delivery over `multipart/mixed` | dropped (deferred by the source itself) | not built; DESIGN §7 does not mention it | the audit's argument stands on HEAD's SPI: `Engine::subscribe` already answers a stream, so `@defer` arrives as a new encoder on an existing shape |
| A7.inproc | in-process binding | built | `ulo::Execution` standalone execution reaches `Engine::execute` directly; the engine tests do this (`ulo-graphql-async-graphql/tests/engine.rs:136-138`) | none |
| A7.rpc | RPC/broker/gRPC carriage | built (as a pattern) | an ordinary handler `#[ulo_rpc::message("gql")] … Dep<dyn Engine> -> GqlResponse` (`ulo-graphql/src/lib.rs:6-10`, DESIGN §7) | no test serves the engine over RPC; the doc example is `ignore` |
| A7.items | the engine answers `Items<Response, E>`, the envelope is the binding's | superseded | `Engine::execute -> BoxFuture<GqlResponse>`, `Engine::subscribe -> BoxStream<GqlResponse>` (`ulo-graphql/src/lib.rs:24-38`); `Items` is an old-core type | the property kept: one response type per call shape, envelope outside it |
| A8.own | three ownership questions: binding (ulo owns), resolver seam (ulo owns), engine (never) | built in part | bindings: owned (`ulo-graphql-http`, `ulo-graphql-ws`). Engine: never owned. Resolver seam: not owned (D3) | the sizing numbers that settle "never write an engine": async-graphql 35,125 lines, juniper 42,572, 23 validation rules in async-graphql's `rules/` alone (memo §8) |
| A8.seam | "the engine executes down to the smallest unit the framework can name": the resolver invocation | superseded | HEAD's unit is the request: the engine executes the whole document, and ulo reaches resolvers only through the context and `dep` (`ulo-graphql-async-graphql/src/lib.rs:106-119`) | see D3 |
| A8.nest | Nest owns schema generation and the transport, not execution | dropped | a framing argument with no artifact at HEAD | none |

### 3.5 Probes and the two corrections (§9)

The six probes are written against the old API (`UloFactory::create_application_context`,
`ulo::di::Execution::Standalone`, `#[injectable(scope = "execution")]`, `ulo::enhancer::Guard`), none of
which exists on this branch; none compiles at HEAD. The claims about async-graphql's own behaviour
are library facts and still hold against 7.x as far as reading goes; the claims about ulo's DI were
about the old container.

| ID | What it proves | Status | Evidence at HEAD | Value to extract |
| --- | --- | --- | --- | --- |
| P1 `probe_resolver_seam.rs` | a `dynamic::Field::new` closure, built once at schema build, reads the request's execution from `Request::data`, runs a guard against it, resolves a two-level provider graph in that execution, reads an argument; the refusal renders at the field position | dropped (old API) | the transport mechanism is built: HEAD inserts `ExecutionRef` with `Request::data` and resolvers read it (`ulo-graphql-async-graphql/src/lib.rs:111-118,156`). The guard half is not built | — |
| P2 `probe_resolver_seam2.rs` | (a) two sibling fields in one document share one execution-scoped instance, a second request builds another; (b) an interceptor with boxed `next` fits the closure; (c) `SubscriptionField::new` takes the same closure shape and its stream is `'static` without a spawn or channel | dropped (old API) | (a) is built and tested for the context: one context per execution shared by every resolver (`ulo-graphql-async-graphql/tests/engine.rs:273`); (b) not applicable; (c) HEAD's async-graphql subscribe spawns nothing (`lib.rs:78-89`) | — |
| P3 `probe_resolver_race.rs` | 24 concurrent sibling fields on an 8-worker runtime, eight rounds, exactly one execution-scoped build per execution; self-tested: `scope = "transient"` gives 24 builds and fires the assertion | dropped (old API) | not re-run on the rebuilt container. The test design is the value: 24 sibling fields, `yield_now` before and after the resolve to widen the window, assert builds-per-execution `== 1` and distinct serials `== 1`, and keep the transient control | — |
| P4 `probe_error_path.rs` | `Error::new_with_source(ShopError)` is recovered by `ServerError::source::<ShopError>()` and `ulo::Error::kind()` reads `NotFound` off it; control: a source-less error answers `None`; nested objects return through `FieldValue::owned_any` and `parent_value.try_downcast_ref` | dropped (old API) | the source slot is used at HEAD only to keep a lookup error on a withheld message (`ulo-graphql-async-graphql/src/lib.rs:113-118`); no error handler reads it, so a resolver error reaches no `#[catch]`-equivalent | — |
| P5 `probe_sub_error.rs` | where the typed source survives on the stream path: query via `execute_stream`, subscription setup failure, mid-stream item failure, validation failure (no source) | dropped (old API) | output was printed, not asserted; the memo records only that the source survives with a Subscription root present (correction 1) | — |
| P6 `probe_isolate.rs` | the round-4 "source lost through `execute_stream`" was the dynamic schema checking `subscription_root()` before the operation type: a query to `execute_stream` on a schema with no Subscription root answers `Subscription root not found` and never executes | superseded (HEAD uses the static schema; the library fact holds) | read at `async-graphql-7.2.1/src/dynamic/schema.rs:299-307,422`; HEAD uses the static `Schema<Q, M, Sub>`, which branches on operation type (`async-graphql-7.0.17/src/schema.rs:598`) | — |
| C9.1 | correction 1: the typed error is not lost through `execute_stream` | superseded (as P6) | the static schema is not affected | the method lesson stands: a later probe falsified an earlier one by flipping one variable, and the claim's confidence marking showed which statements fell |
| C9.2 | correction 2, a binding requirement: graphql-transport-ws and graphql-sse carry queries and mutations ("single result operation"), so a binding routes by operation type | built | async-graphql's static `execute_stream` runs a query once; juniper's `drive` falls back to `execute` on `GraphQLError::NotSubscription` (`ulo-graphql-juniper/src/lib.rs:182-187`); both engine tests send `{ answer }` through `subscribe` and get one response (`ulo-graphql-async-graphql/tests/engine.rs:333-335`, `ulo-graphql-juniper/tests/engine.rs:243-245`) | if Path B is ever built on `async_graphql::dynamic`, this is the first case it meets: the dynamic schema does not route a query on the stream path |
| A9.untested | the macro and the Rust-to-GraphQL type mapping were not tested | dropped | Path B not built | none |

### 3.6 Decisions D1-D8

| ID | What | Status | Evidence at HEAD | Value to extract |
| --- | --- | --- | --- | --- |
| D1 | conform fully per binding; the media type is the gate; `294` collapse rejected | built, with losses | media type negotiated and gating the status (`controller.rs:128-155`). Not built: `422`, `406`, `294`, and the legacy path keeps no status (N1-N3) | the D1 argument, kept: §6.4 itself gates `294` behind `application/graphql-response+json` ("we only recommend its usage alongside the `application/graphql-response+json` media type"), so a collapse to `200` invents a gate the spec supplies. The draft's exposure was weighed as one status code |
| D2 | the graphql-transport-ws state machine is hand-written | built | `ulo-graphql-ws/src/gateway.rs`; DESIGN §7 "Subscriptions" | A3.lib holds the argument |
| D3 | Path B (ulo-declared resolvers, one closure per field) is the design; Path A (schema as a value) a capability | superseded by Path A only | HEAD is Path A: the user binds an async-graphql `Schema` or a juniper `RootNode` as a value; DI reaches resolvers through an execution-scoped context type `C` and `dep::<T>(ctx)` (`ulo-graphql-async-graphql/src/lib.rs:34-46,106-119`; DESIGN §7 "The context is a type parameter") | see §4 Q4. The rejected framing is a standing lesson: "juniper has no `dynamic` module, which may decide whether juniper survives" was rejected because which crate satisfies a capability is a table row, not a design input |
| D4 | enhancers attach to resolvers | open (design gap) | no per-resolver guard, interceptor, metadata or error handler; on graphql-transport-ws no `Ws` tier runs per operation, decided as DESIGN §13 decision 29 (`transports/DESIGN.md:1597`), documented on `GraphqlWs` (`gateway.rs:57-59`) | P1-P4 are the evidence that per-field enhancers are reachable through `Request::data` without fighting `Field::new`'s `for<'a> Fn(ResolverContext<'a>) -> FieldFuture<'a> + Send + Sync + 'static` bound. Not in the ledger as a gap; recorded as a decision |
| D5 | the request-error taxonomy is the SPI's obligation | built coarser | `Outcome { Executed, RequestError, Failed }` (`ulo-graphql/src/lib.rs:117-125`); `Failed` added by U26 (`transports/DIVERGENCES_2B.md:544-558`) | parse vs validation vs coercion is not carried, so the HTTP binding cannot write the draft's 400/422 split (N1). The audit's per-engine precision note still applies: async-graphql's `parser::parse_query` is public and validation is not, so parse is exact and validation rests on empty `path`; juniper's `GraphQLError` variants (`ParseError`, `ValidationError`, `NoOperationProvided`, `MultipleOperationsProvided`, `UnknownOperationName`, `IsSubscription`, `NotSubscription`, `juniper-0.16.2/src/lib.rs:103-111`) map onto §6.4 one for one |
| D6 | the engine is selected per module, so a capability refusal is a trait bound on `for_root` | superseded | the engine is a DI binding, `X as dyn Engine`, optionally qualified, and `GraphqlModule<Q>` reads `Dep<dyn Engine, Q>` (`module.rs:15-23,174-184`) | the trait-bound argument has no subject at HEAD: no engine capability exists to bound on |
| D7 | capabilities as a `#[non_exhaustive]` struct, not defaulted trait methods | dropped | `Engine` has four required methods and no capability record (`ulo-graphql/src/lib.rs:24-38`); `playground_html -> Option<String>` is the one optional behaviour | the shape rule (a struct read once; adding a field is non-breaking; a defaulted method buys the same at the cost of a method) applies if a capability is ever added; Path B's `builds_schema` would be the first |
| D8 | the SPI takes `ModuleRef`; `UloApplicationContext` stays non-`Clone` | built | both adapters hold the binding module's `ModuleRef` and resolve the context with `module.with_execution(&exec).get::<C>()` (`ulo-graphql-async-graphql/src/lib.rs:45,128`; `ulo-graphql-juniper/src/lib.rs:48,221-226`); calls carry `ExecutionRef` | the rejected alternative, kept: a `Clone` application context would let a resolver run `close`, so one shutdown per handle was enforced by `&mut self` on a non-`Clone` type |

### 3.7 Handoff and build-sequence items

| ID | What | Status | Evidence at HEAD | Value to extract |
| --- | --- | --- | --- | --- |
| H.q1 | audit handoff question: does HTTP match GraphQL-over-HTTP (media type, parse vs execute status, GET vs POST) | built in part | as F182-F186; N1-N3 | none |
| H.q2 | does the gateway implement graphql-transport-ws completely | built | as F187-F189 | none |
| H.q3 | where the two engines differ, whose difference | built | the one remaining difference is documented in both crate headers and the ledger (F464) | none |
| H.known.event | `event_field()` needed a manual `Gateway` impl | built | `GatewaySettings::at("/").event("type")` (`gateway.rs:200-202`) | none |
| H.known.refused | `WsError::Refused` built for graphql-ws's 4406 | superseded | `ConnectRefused::code(4406, ..)` (`gateway.rs:216,423-425`) | none |
| H.known.F4 | duplicate-route panic out of `bind()` | built | two configurations on one path are `StartupError::Configure` naming both routes (`mount.rs:341-356`) | none |
| H.adrA | ADR A, *a GraphQL engine is an SPI; its bindings are ulo's* | superseded | no ADR; transports DESIGN §7 carries the decision | none |
| H.adrB | ADR B, *a resolver is a handler* | dropped | Path B not adopted | the draft scope is D3+D4 |
| H.crate | one `ulo-graphql` crate holding SPI, taxonomy and every binding | built differently | three crates: `ulo-graphql` (SPI), `ulo-graphql-http`, `ulo-graphql-ws`; core names no GraphQL type | the HTTP crate depends on the WS crate (`ulo-graphql-http/Cargo.toml:16`), which DESIGN §7 states |
| H.ledger | the new crate's `coverage_ledger.rs` row | superseded | the old `integration-tests` ledger belongs to master; HEAD's crates carry their own `tests/` | none |
| H.docs | the three docs-site GraphQL pages invalidated by PRs 4-10 | open | the docs site was not part of the rebuild; whether the pages were rewritten is the docs audit's | the page list: `content/docs/graphql/index.mdx`, `async-graphql.mdx`, `juniper.mdx` |
| H.free | only F185 and F193 are free-standing; F184/F186/F194/F190 are owed by the rewrite | built | rows 22 and 23 merged on master (#360, #361); the rest was rewritten | the sequencing rule (do not fix code the rewrite replaces) |
| H.gate | divergence gate: PR 7 unable to answer both bindings from one taxonomy stops the build | open (never exercised) | no SSE binding (A7.sse) | none beyond A7.sse |
| R22 GQL-1 | playground as `text/html` | built | as F185 | none |
| R23 GQL-2 | delete the juniper header comment | built | as F193 | none |
| R123 GQL-3 | SPI crate, `EngineCapabilities`, `RequestErrorKind`, `ModuleRef` handle | built differently | `Engine`, `Outcome`, `ModuleRef`; no capabilities, no per-kind taxonomy | N1 |
| R124 GQL-4 | two engine impls, `GraphQLModule<E>` with `with_schema`, fixed token deleted | built differently | `AsyncGraphql<S, C>`, `Juniper<Q, M, Sub>`, `GraphqlModule<Q>` with `engine_from` | test owed (§2 item 5) |
| R125 GQL-5 | HTTP binding with `406`, `415`, `294` and §1's table as a suite against both engines | built in part | `415` and negotiation built; `406`/`294`/`422` not; the HTTP suite runs against a stub engine (`mount.rs`), the engines are tested through `Engine` only | §1's twelve-row table (memo §1) is the conformance suite still owed end to end against both real engines |
| R126 GQL-6 | WS binding, §3's table as a suite | built | `protocol.rs` (14 cases), `init_refused.rs`, `runtime.rs` | none |
| R127 GQL-7 | graphql-sse | open | as A7.sse | as A7.sse |
| R128 GQL-8 | dynamic-schema builder, probes promoted | dropped with D3 | — | P1-P6 |
| R129 GQL-9 | `#[graphql_object]`, `#[query]`, `#[mutation]`, `#[subscription]`, `#[arg]` | dropped with D3 | — | none |
| R130 GQL-10 | `ContextBuilder` deleted, `with_auth` examples on guards | superseded | no `ContextBuilder`; the context is an execution-scoped injectable reading `Option<Ext<CurrentUser>>` and `Option<Dep<RequestHead>>` (DESIGN §7); examples `examples/31-graphql`, `examples/32-graphql-subscriptions` | authentication moved to the HTTP route's guards and the context, not to per-field guards |

### 3.8 Couplings

| ID | What | Status | Evidence at HEAD | Value to extract |
| --- | --- | --- | --- | --- |
| C.F163 | F182 and F163 are one argument (one status where a standard gives several) | superseded | statuses come from one render mapping on HTTP; the GraphQL controller writes its own (`controller.rs:241-247`) | the correction kept: the GraphQL status was never blocked on the pipeline, because the controller builds its response directly; the first ledger text claiming otherwise was wrong |
| C.DI14 | DI-14 drops C20's GraphQL half to GQL-4 | superseded | no string tokens at HEAD | none |
| C.WS | F190's adapter half to the WS pass (row 89) | built | `ulo-ws/src/connection.rs:787-795` | none |
| C.D7 / S10 | the engine SPI takes the shape of HTTP-1's `AdapterCapabilities`; one capability record across transports | dropped for GraphQL | no engine capabilities (D7) | none |
| C.WS2 | GQL-6 blocked by WS-2 (a refused message closes with its code) | built | the gateway closes with protocol codes directly (`gateway.rs:402-408`) | none |

### 3.9 New at HEAD (found while checking)

| ID | What | Status | Evidence at HEAD | Value to extract |
| --- | --- | --- | --- | --- |
| N1 | every request error is `400`; the draft gives `422` for validation, coercion, an undeterminable operation and a not-well-formed request | open, not in the ledger | `Outcome` has one `RequestError` (`ulo-graphql/src/lib.rs:118-125`), rendered `400` (`controller.rs:152`); DESIGN §7 and REVIEW_2B R36 state "400 being the status for those" | draft 2026-11-01: "If the GraphQL document in the request cannot be parsed, status code `400` SHOULD be used." "If the request does not pass GraphQL validation, status code `422` SHOULD be used." "If the variable values cannot be coerced …, status code `422` SHOULD be used." "If the operation to execute cannot be unambiguously determined, status code `422` SHOULD be used." "If the request is not a well-formed GraphQL-over-HTTP request, status code `422` SHOULD be used." The 2026-09-22 audit read the same four 422s independently. SHOULD, not MUST: HEAD is conformant to the MUST (a `4xx`) and diverges from the SHOULD |
| N2 | the legacy `application/json` path answers `200` for every response | open, not in the ledger | `Media::status` `_ => OK` for `Json` (`controller.rs:149-155`); test rows `mount.rs:146,153` assert `200` for a request error | draft 2026-11-01: a client indicating `application/json` and not the server's preferred type is served "following the requirements of this specification as if the request had specified `Accept: application/graphql-response+json`, except any response that produces a `2xx` status code should replace the `Content-Type` header with `Content-Type: application/json`." The audit's handoff PR 5 verification says the same: "status codes intact and `Content-Type` replaced on `2xx`". The 200-for-everything rule HEAD follows is from an earlier draft's `application/json` section, which the current draft no longer has |
| N3 | no `406`; an `Accept` naming neither JSON type is served legacy JSON with legacy statuses | open, not in the ledger | `Media::negotiate` falls to `Json` (`controller.rs:135`); test row `("text/plain") → 200, application/json` (`mount.rs:153`); DESIGN §7 "with no 406" | draft 2026-11-01: the server "MUST either: 1. Respond with a `406 Not Acceptable` status code and stop processing the request; OR [2.] Disregard the `Accept` header and respond with the server's choice of media type", and "If the `Accept` header is present but does not indicate support for any of the server's supported media types or `application/json`, the server SHOULD respond with `406 Not Acceptable`." HEAD takes option 2, which meets the MUST; combined with N2 it answers a request error `200` to a client that accepts neither JSON type |
| N4 | the no-`Accept` rule cites a "2025-01-01 watershed" | open (doc citation), not in the ledger | `controller.rs:124-127`; DESIGN §7; `DIVERGENCES_2B.md:536` | draft 2026-11-01 has no watershed: "If a client does not supply the `Accept` header then the server may respond with an error, or with any content type it chooses." HEAD's behaviour is permitted; the cited rule is not in the current draft, so the doc comment names a source that no longer says it |
| N5 | init timeout timing on `sleep` | open, ledger holds it | F490 names graphql-ws's `connection_init` timeout among the sites still on `sleep` | none beyond F490 |

## 4. Worth discussing with the designer

**Q1. 400 for every request error, or the draft's 400/422 split (N1)?** The split needs `Outcome` to
carry which pre-execution fault occurred, or a second field beside it. juniper answers it exactly
from `GraphQLError`; async-graphql answers parse exactly and validation by elimination.
*Recommendation:* confirm the draft text first (the 2026-11-01 reading went through a summariser; the
2026-09-22 audit read the same). If it holds, add a `#[non_exhaustive]` request-error kind and render
`422` for everything but a parse failure, since two independent readings of the specification
disagree with what DESIGN §7 attributes to it. `Outcome` is not `#[non_exhaustive]`
(`ulo-graphql/src/lib.rs:117`), so the shape decision is cheaper before release.

**Q2. Legacy `application/json`: 200 for everything, or the spec's statuses with the
`Content-Type` swapped on `2xx` (N2)?** *Recommendation:* the draft's rule. It is what D1 settled
and what the handoff's PR 5 verification asserted; the 200-for-everything reading is an older
draft's, and a legacy client that branches on the status is the case it breaks.

**Q3. `406` for an `Accept` that names neither JSON type (N3)?** Option 2 is conformant, but under
HEAD's N2 rule it hides request errors behind `200`. *Recommendation:* `406` per the SHOULD; if
option 2 is kept, serve `application/graphql-response+json` with its statuses (the server's choice),
not the legacy path.

**Q4. The resolver seam (Path B, D3/D4).** HEAD chose a context-type design: DI reaches resolvers
through one execution-scoped context and `dep::<T>`, authorization sits on the HTTP route's guards,
on graphql-transport-ws's connect guards and inside resolvers (DESIGN §13 decision 29). Path B would
give per-field guards, interceptors, metadata and a typed-error route to error handlers.
*Recommendation:* record the context-type design as the answer and Path B as declined, or as parked
with P1-P4 as its feasibility evidence. A judgment call: per-operation authorization on
graphql-transport-ws is the concrete thing given up, and the designer should state whether that is
intended beyond the doc sentence.

**Q5. graphql-sse (A7.sse).** *Recommendation:* add it to DESIGN §7 as unbuilt-with-reason or as a
planned binding. Its distinct-connections rule (validation errors as `200` plus `next`) is the case
that shows why the status table lives in the HTTP binding, and HEAD's `Outcome` already supports it.

**Q6. Two schemas under qualifiers.** *Recommendation:* add one test binding `dyn Engine` and
`dyn Engine @ Admin` at two paths, each answering its own schema; F191's probe is the falsifier.

**Q7. juniper drops `extensions` (F194).** *Recommendation:* state it where a user chooses an engine
(DESIGN §7 or the HTTP crate's docs), not only in the juniper crate header.

## 5. Not verifiable here

- **P3's race result on the rebuilt container.** Whether one execution-scoped binding is built once
  under 24 concurrent sibling resolvers needs a build; HEAD's context test covers sequential
  sharing only (`ulo-graphql-async-graphql/tests/engine.rs:273`).
- **The draft quotations in N1-N4.** Read through a summarising fetch; confirm against
  `spec/GraphQLOverHTTP.md` on the `graphql-over-http` repository's `main`.
- **A5.dihash.** Whether the rebuilt container refuses an ambiguous import is the DI salvage's.
- **`294` in the draft.** The 2026-11-01 reading returned both a `294` SHOULD for a result with
  `data` and `errors` and a `200` SHOULD for field errors; which applies under which media type needs
  the raw text. HEAD answers `200` for field errors under both media types (`mount.rs:142,147`).
- **P5's printed results.** The probe asserted nothing; its outcomes for setup failure, mid-stream
  failure and validation failure on the stream path are not recorded in the memo and need a run.
