# The GraphQL audit

`ulo-graphql-async-graphql` and `ulo-graphql-juniper` against the specifications that own the
surface. The brief is `GRAPHQL_AUDIT_HANDOFF.md`; its scope line reads *"the question is what ulo
adds, what it passes through, and where the two diverge from each other and from the spec."*

The pass ran in two halves. The first read the two crates against **GraphQL over HTTP** and
**graphql-transport-ws** and filed what it found. The second, opened by the user on the same day,
asked a question the brief did not: *what should ulo own here at all?* That question reframes the
first half, and §7 through §10 are its answer. Nothing below §6 was part of the original brief.

Out of scope by the brief: the GraphQL language, and the enhancer pipeline.

## Where the findings live

`FRAMEWORK_GAPS.md`, **F182–F195**. This pass was invoked as an audit of the framework, not of the
documentation site, so the whole of it files there — the `CLAUDE.md` rule that origin decides and
subject never moves an entry. F193 is a finding about a comment and files here with the rest.

This memo does not restate them. It says what they add up to and what was decided.

## Not a build

Nothing was changed. No branch exists. Every probe was deleted from the tree and the manifests they
needed were restored; `git status` is clean at `654c7183`. The six probe files are kept in the
`artifacts/resolver-seam-probes/` beside this memo, promotable to tests if §10's design is built.

## Confidence marking

- **Probed** — run this session against master `654c7183`, over axum where a server was needed.
- **Read** — read off the source, cited by file and line.
- **Spec** — the published document's own words, quoted rather than paraphrased.
- **Unverified** — believed, not established. Marked inline and not acted on.

Both specifications were fetched rather than recalled: GraphQL-over-HTTP from the `graphql-over-http`
repository's `spec/GraphQLOverHTTP.md`, graphql-transport-ws and graphql-sse from their `PROTOCOL.md`
files. One claim in a fetched summary — a `294` status code — read as a fabrication and was checked
against the source before use; it is real, and it is §6.4's recommendation for partial success.

**Two probe results in this memo were corrected by later probes, and both corrections are recorded
where the claim sits.** §9 carries them. The marking is what made each tractable: the claim named
what it rested on, so falsifying the premise showed exactly which statements fell.

## The shape of it

**Two conformance problems, and one question underneath them that decides how both are fixed.**

The first problem is that neither integration conforms to GraphQL-over-HTTP, and they fail in the
same places because the second crate's controller is a copy of the first's. §5 and §6 of that spec
decide four things — the response media type, `Accept` negotiation, the request `Content-Type`, and
the status code — and ulo reads none of the three headers and writes one status. The spec has a name
for the result: *"A legacy server is a server that does not support responses using the
`application/graphql-response+json` media type, and thus does not conform to this specification."*
F182, F183, F184 and F186 are one seam seen four times; F185 and F194 sit beside them.

The second is that the subscription gateway implements graphql-transport-ws's message types and none
of its rules. The protocol specifies five closures and the gateway implements zero, because it holds
no connection state: a map of `connection_init` payloads, written only when the payload was
non-empty, cannot answer *"was this connection acknowledged"*. F187, F188 and F189 reduce to that.
F190 is separate and is the one a user meets first.

Underneath both: **each integration reimplements what its library already ships, and every
divergence is in the reimplementation.** That observation is what §7 through §10 act on. The
sections are independent up to §6 and cumulative after it.

## 1. The HTTP surface answers one status and one media type — F182, F183, F184, F186

Probed at `654c7183`, both crates, axum. What the spec gives against what ulo answers:

| Case | Spec | async-graphql | juniper |
| --- | --- | --- | --- |
| document does not parse | `400` §6.4 | `200` | `200` |
| document fails validation | `422` §6.4 | `200` | `200` |
| variables do not coerce | `422` §6.4 | `200` | `200` |
| `variables` is a string, not a map | `422` §6.4 | `200`, variables dropped | `200`, variables dropped |
| no `query` key | `422` §6.4 | `400` | `400` |
| body is not JSON | `400` §6.4 | `400` ✓ | `400` ✓ |
| field error, data present | `200`/`294` §6.4 | `200` ✓ | `200` ✓ |
| `Content-Type: text/plain` | `415` §6.4 | `200`, executes | `200`, executes |
| no `Content-Type` | `4xx` §5.4 | `200`, executes | `200`, executes |
| `Accept: …graphql-response+json` | that type §6.1 | `application/json` | `application/json` |
| `Accept: text/csv` | `406` §6.4 | `200 application/json` | `200 application/json` |
| `DELETE` on the path | `405` + `Allow` | `405`, `Allow: POST,GET,HEAD` ✓ | same ✓ |

The `405` row is the adapter's and is correct; nothing is owed there.

**The discriminator problem, which is why this is not a status literal.** §6.4 keys on *"the GraphQL
result does not contain the {data} entry"*, and `async_graphql::Response` declares `pub data: Value`
with no `skip_serializing_if` (`async-graphql-7.2.1/src/response.rs:10-13`), so a parse error
serialises as `{"data":null,…}` — the test the spec names cannot be run against the library's wire
form. `Response::is_ok()` does not separate them either: probed, a field error and a validation error
both answer `false`. What is available on that side is `errors[].path`, non-empty for a field error.
juniper does not have the problem: `GraphQLError` is `ParseError` / `ValidationError` /
`NoOperationProvided` / `MultipleOperationsProvided` / `UnknownOperationName` / `IsSubscription` /
`NotSubscription` (`juniper-0.16.2/src/lib.rs:103-111`), which maps onto §6.4's list one for one.

That asymmetry is what §10's D5 turns into an SPI obligation rather than a per-crate fix.

## 2. The playground has never worked through a browser — F185

Both crates answer `GET /graphql` with `Body::text(html)`, and `Body::text` sets
`text/plain; charset=utf-8` (`crates/ulo/src/http/body.rs:132-136`). A browser displays the source.
The feature is on by default in debug builds and is what both quick starts tell the reader to open —
`crates/ulo-graphql-juniper/src/lib.rs:67` says *"Visit `http://localhost:3000/graphql` to use
GraphQL Playground!"* — so the crate's own first instruction is the reproduction. One line each.

The same route answers the playground to `GET /graphql?query={ hello }`. GraphQL-over-HTTP makes GET
support a MAY, so not serving queries over GET is a choice ulo may make. Answering a well-formed
GraphQL GET request with `200` and a page of HTML is not that choice, and a client cannot tell the
two apart.

## 3. The subscription gateway has message types and no connection state — F187, F188, F189

Probed at `654c7183` over axum, one case per protocol rule:

| Rule | PROTOCOL.md | ulo | async-graphql's own |
| --- | --- | --- | --- |
| no `connection_init` within the timeout | close `4408` | never closes; no knob | close `3008` |
| a second `connection_init` | close `4429` | second `connection_ack` | close `4429` ✓ |
| `subscribe` before `connection_ack` | close `4401` | executes and streams | close `1011` |
| `subscribe` reusing a live id | close `4409` | two streams, one id | no check |
| a message of an unspecified type | close `4400` | ulo envelope, socket open | close `1002` |
| a request error on `subscribe` | `error`, no `complete` | `next` then `complete` | no `error` either |
| `ping` | `pong` | `pong` ✓ | ✓ |

Three things the table settles that reading alone would not.

**The invalid-message answer is ulo's canonical envelope**, `{"status":"error","kind":"BadRequest",…}`
— a frame with no `type` field. A conforming client closes on receiving it, so the socket does end;
it ends from the wrong side, after ulo has put a non-protocol frame on a protocol-negotiated wire.

**`ServerMessage::Error` is declared and never constructed**, which `#[allow(dead_code)]` on the enum
hides. The comment at `subscription_gateway.rs:305-306` gives that frame as the justification for the
item type being `Infallible` — *"a refusal is already a `graphql-transport-ws` error frame in the
stream"* — so the reasoning rests on a frame that is never written. F189.

**The rightmost column is why delegation was not the answer.** async-graphql's `http::WebSocket` has
the state machine and the init timer ulo lacks and is wrong on three of five codes
(`src/http/websocket.rs:313`, `:329`, `:378`). §10's D2 settles it.

## 4. A client that offers a fallback subprotocol cannot connect — F190

RFC 6455 has a client offer its subprotocols as one comma-separated header. Two places read it as a
single token: the three WS-capable adapters offer the whole raw value back as the one supported
protocol, and the gateway compares it for equality. Probed: offering
`graphql-transport-ws, graphql-ws` failed at the client with *"SubProtocol error: Server sent no
subprotocol"* — before the gateway ran, because axum tests the combined string for membership in the
parsed list and selects nothing. Offering `graphql-transport-ws` alone connects and is echoed.

The adapter half reaches every gateway and belongs with the WebSocket pass; the gateway half is one
comparison.

## 5. One fixed DI token across every schema and both crates — F191

`"GraphQLService"` is a literal in six places across the two crates and exported by both modules.
ADR-0028 rules tokens are `token_of::<T>()` strings; this is neither that nor a `provider_token!`,
and a consumer writes `#[inject("GraphQLService")]` where a bare `#[inject]` by type does not
resolve. The token does not vary with the schema, so two GraphQL modules both answer it.

Probed, and sharper than the reading: with `imports: [alpha(), beta()]` the application panicked —
`Failed to downcast 'GraphQLService' to AlphaSvc` — and with the same two written `[beta(), alpha()]`
it started and handed the consumer the other module's service without a diagnostic. The winner is
neither first-declared nor last: `resolve_from_imported_modules` returns the first match while
iterating an `FxHashSet` (`instance_loader.rs:656-680`, `container.rs:428`).

The mounted endpoints are unaffected — probed, two schemas at two paths each answer their own, a
controller resolving module-locally first. The existing `two_paths_mount_two_modules` asserts only
that `create` succeeds, which is why this was open.

The general defect is the DI pass's. What is this subject's is that a fixed token makes the
ambiguity certain rather than possible.

## 6. The two integrations are one implementation, copied — and both libraries ship what was rewritten

`graphql_controller.rs` is the same file twice: the same request struct, the same `take_request`
guard, the same `collect().await` and `from_slice` pair, the same two 400s with the same message
strings, the same `.expect("Failed to downcast to GraphQLService")`, the same `status: 200`, the same
playground controller. §1's divergences are identical across the two crates for that reason.

Beneath that, each crate reimplements its library. juniper ships `juniper::http::GraphQLRequest` and
`GraphQLResponse`, which carry the request parameters — `extensions` included, F194 — and expose
`is_ok()`; ulo hand-builds `{"data": result}` and adds `errors`. async-graphql ships
`async_graphql::http::WebSocket` with `Protocols`, `on_connection_init`, `on_ping` and
`keepalive_timeout`; ulo hand-rolls the message enums, the subprotocol string and the stream
plumbing. It also ships `Executor`, a trait whose `execute_stream` returns `BoxStream<'static, _>`
(`src/executor.rs:56-60`) — the lifetime the gateway worked around with `tokio::spawn` and an mpsc
channel (`subscription_gateway.rs:264-277`) was never a constraint on the trait.

Of the four things §1 and §3 are about — request parsing, response shape, the WS state machine, the
subprotocol constant — the libraries implement four, ulo implements four, and they disagree on all
four.

---

*Everything above was the brief. Everything below answers the question the user opened after it.*

---

## 7. Conformance is per-binding, and the bindings contradict each other

GraphQL is transport-agnostic. The core exchange is `(query, operationName, variables, extensions)`
in and a result out, and anything that carries those carries GraphQL. Three transports have
published protocols; the rest is unspecified carriage.

**The fact that decides the design.** graphql-sse's distinct-connections mode forbids what
GraphQL-over-HTTP requires:

> "Validation steps that run before execution of the GraphQL operation MUST report errors through an
> accepted SSE connection by emitting `next` events that contain the errors in the data field...
> Additionally, responding with a `400` (Bad Request) will cause the user agent to fail the
> connection."

A validation error is `422` on the HTTP binding and MUST be `200` plus a `next` event on the SSE
binding. Same engine, same error, two conformant answers that contradict each other.

**So no status rule can live in a GraphQL layer.** It lives in each binding. A design that put §1's
table anywhere shared would produce a non-conformant SSE binding by construction.

The bindings in view, and who has one today:

| Binding | Specified by | ulo today |
| --- | --- | --- |
| GraphQL over HTTP | the spec, Stage 2 draft | §1's table |
| graphql-transport-ws | `graphql-ws`'s PROTOCOL.md | §3's table |
| graphql-sse, two modes | `graphql-sse`'s PROTOCOL.md | none |
| HTTP `multipart/mixed` | an `graphql-over-http` RFC | none; async-graphql ships the encoder |
| in-process | nothing | `Execution::standalone()` already is one |
| RPC, brokers, gRPC | nothing — ad-hoc carriage | none; ulo has eight such transports |

**What `Items` already gives.** ADR-0049's `Items<T, E>` doc says *"What varies beside the count — a
status and headers on HTTP, metadata on gRPC — is the answer's envelope and sits outside this"*
(`crates/ulo/src/dispatch/items.rs:9`). That is this seam exactly: `Items<Response, E>` is the
engine's answer, the envelope is the binding's. A query is `One`, a subscription is `Many`,
`@defer` is `Many`. Incremental delivery is Stage 2 and not in the September 2025 spec edition, so
it is not built for — but because a subscription already forces `Many`, it arrives later as a new
encoder on an existing shape rather than a change to the SPI.

## 8. What ulo owns — three questions, not one

"Own the syntax like Nest" is three separable things, and Nest owns two of them. Its decorators
generate a schema; `graphql` is a peer dependency that the Apollo or Mercurius driver executes.

| | Decision | Why |
| --- | --- | --- |
| **Transport binding** — status rules, ws state machine, SSE modes | **ulo owns it** | §1, §3 and §7. The libraries are wrong here and the bindings contradict each other, so the choice has to be made per binding by whoever owns them |
| **Resolver seam** — a resolver as a ulo handler with DI, guards, interceptors, metadata | **ulo owns it** | `async_graphql::dynamic::Field::new` takes `Fn(ResolverContext) -> FieldFuture`, the closure seam Nest builds on. Probed in §9 |
| **Execution engine** — parse, validate, execute, introspect, coerce | **ulo never owns it** | 35,125 lines in async-graphql, 42,572 in juniper, 23 validation rules in async-graphql's `rules/` alone |

The HTTP audit's seam rule transfers verbatim. There it reads *"an adapter routes down to the
smallest unit the framework can name, and no further."* Here: **the engine executes down to the
smallest unit the framework can name, and no further.** That unit is the resolver invocation.

**Two paths, not one.** Path B is ulo's declaration, where the macro collects field metadata and
ulo registers one closure per field that enters its pipeline. Path A is the schema handed over as a
value, which is what both crates do today. They are not exclusive: B is a capability an engine
declares, A is what an engine that declares nothing still supports. Under B the `ContextBuilder`
disappears, because the gap it bridges — the only seam by which ulo's DI reaches a resolver — stops
existing. That is why both `with_auth.rs` examples authenticate there.

**The A/B choice does not fork the bindings.** Both paths end at an engine producing
`Items<Response, E>`, so the HTTP, ws, SSE and RPC bindings are identical under either. That is most
of the work, and it is buildable before A/B is settled.

## 9. The probe — what the design was tested against

Six probe files, run at `654c7183`. The question was whether the resolver seam survives Rust's
bounds: `Field::new` wants `for<'a> Fn(ResolverContext<'a>) -> FieldFuture<'a> + Send + Sync +
'static`, built once at schema-build time, while the execution is per-request.

**It does.** The mechanism is carrying ulo's `Execution` through async-graphql's request `Data` and
reading it back inside the closure — the same plumbing `ContextBuilder` does today, demoted from a
user seam to an internal one.

- **DI, guard, argument, handler.** A closure resolved `HeroService` (itself holding an injected
  `HeroRepo`) at the request's execution, ran a `Guard`, extracted an argument and called the method.
  The guard's refusal rendered at the right field position.
- **Execution scope holds.** Two sibling fields in one document shared one execution-scoped provider;
  a second request built a new one. Under 24 concurrent sibling fields on a multi-threaded runtime,
  eight rounds, exactly one build per execution. **The check was self-tested**: the same probe with
  `scope = "transient"` produces 24 builds and fires the assertion, so the clean run is evidence.
- **Interceptors fit**, boxed `next` and `#[async_trait]` indirection included.
- **Subscriptions keep the shape.** `SubscriptionField::new` takes the same closure, and the returned
  stream is `'static` on its own — no `tokio::spawn`, no channel. Path B would remove the crate's
  `tokio` dependency.
- **Typed errors round-trip.** `Error::new_with_source(ShopError)` is recovered by
  `ServerError::source::<ShopError>()`, and `ulo::Error::kind()` reads `NotFound` off it — the same
  private-source-slot mechanism ADR-0039 built for `GrpcStatus`. So `#[catch(MyError)]` is reachable
  from a resolver. Control: an error with no source answers `None`.
- **Non-scalar returns work**, via `FieldValue::owned_any` and `parent_value.try_downcast_ref`.

**Correction, recorded because a later probe falsified an earlier one.** A round-4 result read as
*"the typed error is lost through `execute_stream`"*. It was not: the dynamic schema checks
`subscription_root()` **before** `prepare_request` and before the operation-type branch
(`async-graphql-7.2.1/src/dynamic/schema.rs:422-428`), so a query sent to `execute_stream` on a
schema with no Subscription root answers `{"errors":[{"message":"Subscription root not found"}]}` and
never executes. Isolated by flipping that one variable.

**That is a binding requirement.** graphql-transport-ws and graphql-sse both carry query and
mutation over the streaming transport — the protocol calls it a *single result operation*. A binding
must therefore route by operation type, `execute` for query and mutation and `execute_stream` for
subscription, rather than sending everything to `execute_stream` as async-graphql's own WebSocket
does. The **static** schema handles this itself (`src/schema.rs:606`); the **dynamic** one does not.

**Not tested:** the macro, and the Rust-type-to-GraphQL-type mapping. Both are engineering if the
closure works and moot if it does not.

## 10. Decisions — settled 2026-09-22

**D1 — conform fully, per binding. The media type is the gate.**
Settled: conform end to end, and where legacy behaviour must survive it is gated rather than
blended. The gate needs no ulo invention, because §6.4 already specifies one — *"we only recommend
its usage alongside the `application/graphql-response+json` media type, which makes the meaning
explicit."* Negotiate the media type, and the status rules follow from which one was chosen; a
legacy client asking for `application/json` gets §6.1's legacy path. **A recommendation to collapse
`294` to `200` was offered and withdrawn**: it invented a gate the spec supplies. The draft status
was weighed and accepted — GraphQL-over-HTTP is Stage 2 and *"not yet official"*, but §6.4's status
table and §6.1's media-type rules have been stable through the draft, and the unsettled parts are
`294` (a code the working group minted itself, unregistered with IANA) and incremental delivery. The
exposure is one status code, already gated.

**D2 — hand-rolled protocol, and it is no longer a decision.**
Settled: ulo writes the graphql-transport-ws state machine. Delegating to
`async_graphql::http::WebSocket` was tempting only while ulo did not own the binding; §8 removes the
premise, and §3's rightmost column shows delegation trading a conforming implementation for one that
is wrong on three of five close codes.

**D3 — Path B is the design, Path A is a capability.**
Settled per §8. The engine declares what it can back; an engine that backs neither the resolver seam
nor anything else registers nothing. **The earlier framing — "juniper has no `dynamic` module, which
may decide whether juniper survives" — was rejected by the user and is wrong**: which crate satisfies
a capability today is a row in a table, not a design input. Recorded because it is the failure mode
[[feedback-a-generic-doc-outlives-the-roster]] names, committed in the document that names it.

**D4 — enhancers attach to resolvers.**
Settled by D3. F195 recorded the choice as *"take enhancer tokens on the module, or strike the
feature line"*; under Path B neither is the answer. A resolver is a ulo handler, so it carries
`#[use_guards]`, `#[use_interceptors]`, `#[set_metadata]` and reaches `#[catch]` the way an HTTP
handler does — probed in §9. The advertised feature becomes true rather than struck.

**D5 — the request-error taxonomy is the SPI's obligation, not a per-crate fix.**
Settled: the engine SPI carries the distinction §6.4 keys on, because §1 shows it cannot be read off
a serialised response. juniper answers it exactly from `GraphQLError`; an async-graphql
implementation reconstructs it — `parser::parse_query` is public, validation is not, so parse
failures are exact and validation failures rest on `errors[].path` being empty. That the two engines
answer with different precision is what an SPI is for.

**D6 — the engine is selected per module.**
Settled. The deciding argument is that it makes a capability refusal a **trait bound** rather than a
startup error: with the engine on the module, `for_root()` can be written
`impl<E: GraphqlEngine + BuildsSchema> GraphQLModule<E>`, and an engine that cannot build a schema
has no such method. App-level cannot do this — `use_graphql_engine` would run after the module is
constructed, leaving nothing to bound against and deferring the check to bind. Supporting: an engine
owns no process-wide resource, so app-level would forbid two engines coexisting at no benefit, and
the DB modules are the precedent for a per-module backend. The motive that makes
`use_http_adapter` app-level does not transfer, because under §8 the binding decides observable
behaviour and the binding is ulo's. A hybrid of app default plus module override was rejected: it
buys one helper function's convenience and costs a resolution rule. Under Path B `Query`, `Mutation`
and `Subscription` stop being type parameters, leaving `GraphQLModule<E>` with one where juniper's
carries five.

**D7 — capabilities are a struct, not defaulted trait methods.**
Settled, following the HTTP audit's own correction. `streams_responses` as a defaulted method is
scheduled for deletion there — *"Two methods where there are four: `register_route`,
`register_ws_route` and `streams_responses` go"* — in favour of a `#[non_exhaustive]`
`AdapterCapabilities` read once, because *"Adding a field later is not a breaking change, which is
what the defaulted-method approach was buying and paying a method for."* The engine SPI takes the
same shape from day one rather than repeating the step.

**D8 — the SPI takes `ModuleRef`, and `UloApplicationContext` stays non-`Clone`.**
Settled. A resolver closure needs an owned resolution handle, and `UloApplicationContext` is not
`Clone`, so the probe wrapped it in an `Arc`. Deriving `Clone` was considered and rejected:
`close(&mut self)` runs every module's destroy, before-shutdown and shutdown hooks
(`application_context.rs:223-228`), and `&mut self` on a non-`Clone` type is what enforces one
shutdown per handle — a resolver holding a clone could shut the application down, twice. `ModuleRef`
is the precedent and the answer: `#[derive(Clone)]` over the same `Arc<RwLock<…>>` shape, with a
surface of `get` / `get_by_token` / `resolve` / `resolve_by_token` / `current_module` and no
lifecycle method. Resolvers belong to a module, which makes module-scoped resolution the closer fit
rather than a compromise. **An argument from consistency with the `HttpContext` family was offered and
withdrawn**: the 2026-09-13 structure audit already ruled `UloApplicationContext` a deliberate
non-member of that family, kept because *"Spring, Nest and .NET all mean the app as a DI root"*.
Family consistency was the wrong test, and reaching for it is what
[[feedback-standards-before-internal-consistency]] warns against.

## What is out of scope, and why

- **The GraphQL language, and the enhancer pipeline** — the brief excludes both. D4 touches the
  pipeline only to say where enhancers attach under the new design.
- **F4** — the handoff asked whether the duplicate-route panic still reproduces before anything was
  filed about it. ADR-0052, which was to close it, is not in the tree at `654c7183` (`docs/adr/`
  ends at 0051), so F4 stands as written and nothing was filed.
- **The adapter half of F190** — three HTTP adapters read a list-valued header as one token, which
  reaches every gateway. Owed to the WebSocket pass.
- **The DI ambiguity under F191** — first match wins over an `FxHashSet`, reporting nothing, is the
  DI pass's finding. F191 records only the part a fixed token causes.
- **Incremental delivery** — Stage 2, absent from the September 2025 spec edition, HTTP binding
  still an RFC. Not built for; §7 says why it costs nothing to be ready for.

## Couplings the triage pass should carry

- **F182 and F163 are the same argument on two surfaces.** F163 is extraction answering `400` for
  three conditions RFC 9110 gives three statuses; F182 is GraphQL answering `200` for three
  conditions its spec gives two. Whoever builds the status seam should read them together. **They
  are not blocked alike**: F163 waits on F136's `Custom { status, message }` because the status is a
  literal in a macro. A GraphQL controller builds `HttpResponse` directly over a bare `u16`
  (`crates/ulo/src/http/http_response.rs:6-10`), and F184's `415` and F182's `422` need nothing from
  the pipeline build. An earlier revision of this memo and of the folder README claimed
  F184 was blocked on F136; it is not.
- **F191 is the DI build's ambiguity finding seen from outside.** If the DI build makes an ambiguous
  token a refusal, F191 becomes a startup error instead of a panic. Under D6 the token stops existing
  in its present form, so the two fixes should not both be written.
- **F190's adapter half belongs to the WebSocket pass**, which has not run.
- **D7 rides the HTTP build's PR 1**, which introduces `AdapterCapabilities`. The engine SPI should
  take its shape from whatever that PR settles on rather than inventing a second one.

## What is owed

**A build handoff, which is now writable and was not before.** D1 through D8 settle the shape; what
remains is sequencing and PR boundaries, which is a session of its own.

Two things that handoff should know. **Only two findings are free-standing** — F185 (one
`with_content_type` per crate) and F193 (delete a stale comment). An earlier revision of this
section listed six, adding F184, F186, F194 and F190's gateway half on the grounds that they need
none of the design. They do not need it, and they are still not free-standing: each of them is a
defect in request parsing or in the ws gateway, and both of those are replaced by the bindings.
Fixing them first is work the rewrite discards, and in a pre-release crate with no users there is
nothing to buy with it. They are owed **by** the binding PRs, which is where the handoff records
them. And **the bindings do not wait on Path B** — §8 establishes that both paths end at the same
`Items<Response, E>`, so the HTTP, ws and SSE bindings are the same work either way and are the
larger half.

The six probe files in `artifacts/resolver-seam-probes/` are the falsification evidence for §9 and should be
promoted to tests rather than rewritten.
