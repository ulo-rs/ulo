# Salvage: extraction

**Subject.** What a handler parameter may be and what a failed one answers: `FromContext` and every
extractor built on it (`Json`, `Path`, `Query`, `Body`, `Bytes`, `BodyStream`, `Multipart`,
`Validated`, `Payload`, `Inbound`, `HttpRequest`, `Request`, `Extension<T>`), the `#[body]` /
`#[param]` / `#[query]` markers, the one-body rule, and the failure rendering on the four
transports. Audited 2026-09-22 against master `654c7183`; findings F163–F181 plus appendices to F1
and F84.

**Source read in full** (`docs/experiments/di-redesign/salvage/source/extraction/`):

| file | words |
| --- | --- |
| `EXTRACTION_AUDIT.md` (the memo, §1–§10) | 6,798 |
| `EXTRACTION_AUDIT_HANDOFF.md` (the audit's brief) | 652 |
| `EXTRACTION_BUILD_HANDOFF.md` (PRs 1–16, verifications, couplings) | 1,734 |

Also read: `FRAMEWORK_GAPS.md` F1, F53, F54, F75, F84, F163–F181; `source/BUILD_SEQUENCE.md` rows
24, 25, 64–75, 100, 107 and S1.

**Checked against.** Branch `experiment/di-redesign`, tip `331c35b9`, 2026-11-01. The rebuild's
extraction: `crates/ulo-transport/src/{extract,validate,__private}.rs`,
`crates/ulo-http/src/extract/*.rs`, `crates/ulo-http/src/render.rs`,
`crates/ulo-handler-codegen/src/{params,emit,body}.rs`, transports `DESIGN.md` §2.2, §2.4, §3.1,
§3.6, `transports/DIVERGENCES.md` §Extraction, `SWITCH_INVENTORY.md`. No build or test was run.

**The one fact that decides most rows.** The rebuild replaced `FromContext<C>` with
`FromCall<T: Transport>`, whose failure is one concrete enum, `ExtractError`, rather than an
associated type (`crates/ulo-transport/src/extract.rs:20-33`, `:80-108`). `ExtractError`
implements `std::error::Error` and `Classify`, and the generated call boxes it into the error
handlers' input (`crates/ulo-transport/src/__private.rs:41-52`). Parameters are read through one
trait, `Param<T, M>`, with no spelling read (`crates/ulo-handler-codegen/src/params.rs:1-7`), and
the markers do not exist (`SWITCH_INVENTORY.md:303`). The memo's §1–§5 problems are therefore
mostly gone by construction; what survives is a handful of status choices and the test ideas.

---

## 1. Summary

| status | count |
| --- | --- |
| built | 28 |
| superseded | 28 |
| open | 10 |
| dropped | 15 |
| UNVERIFIED | 1 |
| not graded (another subject's) | 1 |
| **items** | **83** |

"Built" includes three rows built with a stated departure from the decision (F176, D4, EXT-4);
each is also raised in §7. Item 3 below is a superseded row whose replacement lost part of what
the decision bought; it is listed because the loss is unrecorded.

**The five most valuable open items.**

1. **F163 / EXT-3, the 422 half.** A JSON body that parses and does not fit `T` answers 400
   `Malformed`; RFC 9110 §15.5.21 names 422 for it, and `serde_json::Error::classify()` already
   separates the two cases (§3.1). Not in the ledger.
2. **F164.** A body a guard or interceptor already took answers the caller 400
   `` `body` is missing ``, and `Option<Json<T>>` reads it as `None`: the application's fault is
   reported as the client's, or not at all. Recorded as intended in `transports/DIVERGENCES.md:533`;
   not in the ledger as a defect (§3.2).
3. **D1's lost half: a custom extractor cannot name its own failure.** `FromCall` returns
   `ExtractError`, whose kinds are `BadRequest`, `Unprocessable`, `Internal` and whatever a
   container lookup maps to; a custom extractor that means 401 or 404 has no variant for it (§5).
4. **M1 / V9: the spelling matrix has no in-tree test.** The rebuild's claim "an alias means what
   its type means" and its pairwise body assertion are covered by probes under
   `docs/experiments/di-redesign/probes/` only; no `compile_fail` fixture and no accepted-spelling
   suite exists in `crates/`. The audit's method note (each spelling compiled on its own) is the
   recipe (§6).
5. **F178 in its changed form.** `docs/adr/0023` is still `accepted` and describes `FromContext`,
   which does not exist at HEAD; no ADR records `FromCall` / `ExtractError` (§3.4).

---

## 2. The older ledger entries the audit consumed

| ID | what | status | evidence at HEAD | value to extract |
| --- | --- | --- | --- | --- |
| F84 | HTTP extraction failure answers an inline 400 and bypasses the error chain | built | `__private.rs:49-51`: `Param::extract` answers a `BoxError` the generated `?` hands to `dispatch`; `ulo-http/tests/extract.rs:191-235` runs every case through a second app whose error handler reports what the extractor said | The memo's correction (below) |
| F84 correction | The bypass was not "before guards"; guards and the interceptor's before-phase ran first and the after-phase saw the 400 as `Ok` | built | `emit.rs:41`: "Parameters extract in order, inside `dispatch`'s call, so after every guard admits"; a failure now reaches `next`'s caller as `Err` | Probe sequence `["guard", "interceptor:before", "interceptor:after:Ok(400)"]` (memo §1). Test idea kept: a recording guard and interceptor over a failing `Json<T>`; the after-phase must see `Err` |
| F1 | RPC and WebSocket render an extraction failure as `Internal` | built | `ExtractError::classify` answers `BadRequest`/`Unprocessable` (`extract.rs:158-166`); `ulo-rpc-conformance/src/cases/errors.rs:39-43` asserts `bad_request` for an undecodable `Payload<T>` | — |
| F1 gRPC appendix | gRPC lifted every extractor failure to `INTERNAL`, a validation failure included | built | `ulo-grpc/src/status.rs:23`: `BadRequest \| Unprocessable => InvalidArgument`; DESIGN §2.4 table adds `BadRequest` field violations | The rule the appendix states stays true: a handler-side fault (gRPC's old `RequestError`) is `Internal`, a caller's bad message is `InvalidArgument`; F164 below is the HTTP case of the same split |
| F53 | `PayloadError` was two unrelated types under one name | superseded | one `ExtractError` for every transport (`extract.rs:80-108`) | Residual, not a defect: `ulo_rpc::Payload` (`ulo-rpc/src/extract.rs:32`) and `ulo_ws::Payload` (`ulo-ws/src/envelope.rs:126`) are still two types sharing a name, now in separate crates |
| F54 | Only HTTP lets a handler declare what it reads beyond the payload | built | HTTP `Header<H>`, `HeaderMap` (`ulo-http/src/extract/head.rs:26-45`); RPC `CallHeaders` (`ulo-rpc/src/transport.rs:271`); gRPC `GrpcMetadata` (`ulo-grpc/src/extract.rs:92`); WS `Dep<UpgradeHead>`, `ConnectionInfo` (DESIGN §2.10) | The user's 2026-09-14 criterion: an extractor earns its place by the boilerplate it saves over `cx.headers()` |
| F75 | A wrong `Payload<T>` on gRPC failed on the first call, not at compile time | built | `crates/ulo-codegen-tests/tests/ui/grpc_request_of_another_message.rs` (+ `.stderr`) | — |

---

## 3. The audit's findings, F163–F181

### 3.1 Failure status and the error type

| ID | what | status | evidence at HEAD | value to extract |
| --- | --- | --- | --- | --- |
| F163 (415 half) | `Body<T>` refused an unknown media type with 400; RFC 9110 gives 415 | built | `render.rs:75-79` writes 415 with `Accept` naming the expected type; `tests/extract.rs:279-280`; `ulo-http-conformance/src/cases/extraction.rs:9-27` on every host | — |
| F163 (validation row) | A body failing `#[validate]` answered 400; RFC gives 422 | built | `Valid<P>` fails as `Invalid` (`validate.rs:45-64`), classified `Unprocessable` (`extract.rs:161`); conformance `extraction.rs:20` | — |
| F163 (shape row) | `{"wrong":1}` with `application/json`, which lexes and does not fit `T`, answered 400; RFC gives 422 | **open** | `ulo-http/src/extract/body.rs:52` maps every `serde_json` error to `Malformed`, which classifies `BadRequest` (`extract.rs:164`). Not in the ledger | See below |
| F165 | `BodyExtractionError<E>` implemented only `Display`, so the most-used extractor's failure could not reach any `&dyn Error` seam | superseded | no per-extractor error types; `ExtractError` implements `Error` (`extract.rs:144-151`) | — |
| F175 | `Validated<E>` flattened the inner error and `ValidationErrors` into strings | built | the inner failure propagates as itself (`validate.rs:54-57`); violations travel as `Detail::FieldViolations` (`extract.rs:176-181`); `validator` reaches through `validator_bridge::violations`, which drops the rejected `value` parameter as possibly secret (`validate.rs:97-144`) | — |

**F163, shape row: value to extract.** The memo's live table (§2), sent through a server at
`654c7183`:

| request | ulo answered | RFC 9110 |
| --- | --- | --- |
| `application/json`, body `<<<not json>>>` | 400 | 400 (§15.5.1) |
| `text/plain`, body `{"name":"ada"}` through `Body<T>` | 400 | 415 (§15.5.16) |
| `application/json`, body `{"wrong":1}` | 400 | 422 (§15.5.21) |
| a body that fits `T` and fails `length(min = 3)` | 400 | 422 (§15.5.21) |

At HEAD rows 1, 2 and 4 match the RFC; row 3 does not. RFC 9110 §15.5.21, quoted by the memo:
*"the server understands the content type of the request content (hence a 415 … is
inappropriate), and the syntax of the request content is correct, but it was unable to process the
contained instructions."* A missing field or a wrong-typed value in syntactically valid JSON is
that case. The distinction is available without a parser change: `serde_json::Error::classify()`
answers `Category::Syntax` or `Category::Eof` for a body that does not lex and `Category::Data` for
one that lexes and does not fit, so `Json<T>` can answer `Malformed` for the first and a 422-kinded
failure for the second. `ExtractError` has no variant for "decoded but did not fit"; `Invalid` is
the 422 variant and carries `FieldViolation`s, which a serde `Data` error could fill with one entry
naming the field when serde names one. `Form`, `Query` and `Path` share the same question
(`de.rs:71-80` answers `Missing` or `Malformed` only). Whether 400 is kept on purpose is §7 Q1.

### 3.2 F164: a body read twice

| ID | what | status | evidence at HEAD | value to extract |
| --- | --- | --- | --- | --- |
| F164 | A body already read by something earlier answered the caller 400, where the source's own comment called it the handler's fault | **open** (as a judgment call; built as a stated choice) | `HttpCx::take_body` is public (`ulo-http/src/cx.rs:133-136`), so a guard or interceptor holding `&HttpCx` can take it; `take_present` answers `Missing { param: "body" }` for a taken body (`extract/body.rs:222-231`), classified `BadRequest`; `Option<P>` turns `Missing` into `None` (`extract.rs:39-53`). `transports/DIVERGENCES.md:531-533` records "a body already taken (a guard's `take_body`) is `Missing` for all five". Not in the ledger | See below |

**Value to extract.** The old tree's comment at `crates/ulo/src/extract/from_context.rs:97-99`
(`654c7183`): *"The client's request was fine; the handler asked for the body twice"*, logged at
`error`. gRPC's `RequestError` doc at the same commit (`crates/ulo/src/grpc/runtime.rs:460-462`)
stated the rule: *"Each is a fault in the handler or the dispatch rather than in the call, so the
generated method answers `Internal`."* The rebuild's form is sharper than the old one in two ways:

- The client is told `` `body` is missing `` (`extract.rs:127`) about a request that carried one.
- `Option<Json<T>>` reads a consumed body as `None`, the same value it gives a request with no
  body, so a guard that consumed the body turns the handler's optional-body branch on with nothing
  logged. The old `Option` arm swallowed every error; the rebuild's narrowing to `Missing` is
  correct for absent input and wrong for this one case, because "taken" and "absent" share a
  variant.

The fix the old record proposed carries over: a distinct cause for "already taken" (a variant, or
`Missing` keeping a reason) that classifies `Internal`, is logged at `error`, and that `Option<P>`
does not swallow. The test the build handoff owed (PR 3): a guard reads the body, the handler takes
`Json<T>`, the answer is 500. §7 Q2.

### 3.3 Media types

| ID | what | status | evidence at HEAD | value to extract |
| --- | --- | --- | --- | --- |
| F176 | `Json<T>` ignored `Content-Type` while `Body<T>` refused anything without `application/json`, including every `+json` type | built, with one departure | `Json` takes `application/json` and any `+json` suffix, essence lowercased (`extract/body.rs:233-243`); `tests/extract.rs:275-279` covers `application/json`, `; charset=utf-8`, `application/vnd.api+json`, `text/plain`. Departure: an absent `Content-Type` answers 415 (`body.rs:48`, `DIVERGENCES.md:534`), where D4 chose "unknown-and-try" | See below |
| A4 (docs ledger) | An empty body raised `NotJson`, which said the body was not JSON | built | an empty body is `Missing { param: "body" }` (`body.rs:225-231`); `tests/extract.rs:282` | — |

**F176: value to extract.** Spec text the memo fetched: RFC 6838 §4.2.8 formalises the `+suffix`
convention; the rebuild's comment cites RFC 6839, which registers `+json` (§3.1 there). Both are
correct; RFC 6839 is the more specific. `application/problem+json` is the type the rebuild's own
error bodies use (DESIGN §2.4), so `Json<T>` reading it matters for a service consuming another
ulo service's errors. The PR 4 verification list from the build handoff is broader than the test at
HEAD: it also names `application/problem+json` and **no header**, neither of which
`tests/extract.rs` sends. On the absent header: RFC 9110 §8.3 lets a recipient without
`Content-Type` *"assume a media type of `application/octet-stream` or examine the data to
determine its type"*. 415 is the first reading and D4's "try" the second; both are permitted, and
the choice is §7 Q3.

### 3.4 The record

| ID | what | status | evidence at HEAD | value to extract |
| --- | --- | --- | --- | --- |
| F178 | ADR-0023 says the compile-time one-body check is impossible; ADR-0040 built it; 0023 carries no pointer | **open**, premise changed | `docs/adr/0023-extraction-is-one-trait.md:3` still reads `Status: accepted` with no pointer; the whole record describes `FromContext`, which does not exist on this branch, and no file under `docs/adr/` mentions `FromCall` or `ExtractError`. Not in the ledger | The rebuild's body check is the same mechanism ADR-0040 introduced, now `CONSUMES_BODY` read through `Param<T, _>` in a `const` (`ulo-handler-codegen/src/body.rs:27-52`, transports DESIGN §2.2). 0023's *Roads not taken* argument ("neither survives being raised from inside a macro's own output") is refuted twice over; any ADR series that survives the switch either supersedes 0023 or retires it. §7 Q5 |

### 3.5 HTTP's two extraction languages, and spelling reads

All of these concerned the old `controller_macro` and `markers_params` code, which has no
counterpart at HEAD. Every parameter type goes through `<Ty as Param<T, _>>::extract`, emitted
once per parameter in signature order (`ulo-handler-codegen/src/emit.rs:189-221`), and
`params.rs:1-7` states that no parameter is classified by its spelling.

| ID | what | status | evidence at HEAD | value to extract |
| --- | --- | --- | --- | --- |
| F166 | `ExtractorKind`'s thirteen variants made four distinctions; one mistake yielded one of four diagnostics by the neighbour's name; context refs read by name on all four transports (R1, R2, R13–R15) | superseded | `Param<T, M>` with `M` inferred (`__private.rs:41-52`); the context is a parameter by value through `impl FromCall<Http> for HttpCx` (`head.rs:68-72`) and its peers on RPC, WS and gRPC | Residual name read: the receiver `self: Arc<Self>` is recognised by the last segment `Arc` (`params.rs:99-108`), and an alias of `Arc` is refused (transports DESIGN §2.2 line 110). That is the one spelling read left in parameter analysis, and the refusal is stated rather than silent |
| F167 | A parameter whose pattern is not an identifier (`_: Json<Dto>`, `Point { x, y }: Point`) was dropped from the generated call, failing E0061 | built | the call binds generated idents `__ulo_argN` and passes them to the method, so the pattern stays in the method's own signature (`emit.rs:195-213`); diagnostics name a pattern by its tokens (`params.rs:110-115`) | No in-tree handler uses `_:` or a struct pattern (searched `crates/`); the two spellings the memo probed are the test to add (§6) |
| F168 | `HttpRequest` had a macro arm and a trait impl answering the client different messages | superseded | no whole-request extractor; `ulo_http::Request` is the pre-dispatch middleware's request type (`ulo-http/src/lib.rs:118`); a handler takes `HttpCx`, `Bytes` or `BodyStream` | — |
| F169 | `#[query("name")]` on a non-scalar dropped the name | dropped | markers do not exist (`SWITCH_INVENTORY.md:303`; `ulo-http-macros/src/lib.rs` exports verbs only) | — |
| F170 | `is_scalar_type` read primitive names, so `type Count = usize` changed the marker's mechanism | dropped | as F169 | The class (a name read where no downstream type check catches a wrong branch) is the one to keep watching; the `Arc` receiver above is its only instance in parameter analysis |
| F177 | Every route wrapper cloned the request parts for a branch most handlers did not take | superseded | `HttpCx` holds `Arc<RequestHead>` (transports DESIGN §3.1 line 433) | — |
| F179 | An attribute before a marker turned the marker off | dropped | markers gone; `params.rs` reads no parameter attributes | A new edge the same fact opens is N1 in §8 |
| A2 | `#[param]`'s `default` parsed and ignored | dropped | markers gone | D3's argument, which `Path<T>` now enforces (§4) |
| A3 | A bad `#[query]` default panicked per request with the panic text sent to the client | dropped | markers gone; `Query<T>` defaults are serde's | — |
| C4 | A `#[query]` default turned an unparseable value into an accepted one | superseded | serde semantics: a missing field takes `#[serde(default)]`, an unparseable one is `Malformed` (`de.rs:71-80`, `DIVERGENCES.md:522-528`) | — |
| A15 / C2 | `#[query("name")]` emitted an unresolved path; once fixed, the name was dropped | dropped | as F169 | — |
| A16 | A marker and a typed extractor could not share a signature | dropped | one extraction language | — |
| A17 | A marker beside `HttpRequest` emitted `__req`, bound by nothing | dropped | as F168 | — |
| C3 | Markers spoke six error strings and nine hardcoded 400s beside the typed path's one shape | superseded | one `ExtractError` shape rendered once (`render.rs:59-79`); the old marker path's virtue, naming the parameter, is kept: every variant carries `param` (`extract.rs:110-122`) | The memo's side-by-side (§3): `Path<i32>` named the whole map, `#[param("id")] i32` named `id` and the parse error. The rebuild's `Path` names the field (`tests/extract.rs:255`) |
| C23 | `ExtractorKind` drifted from its purpose | superseded | as F166 | — |
| C24 | `HttpRequest` had two implementations | superseded | as F168 | — |

### 3.6 WebSocket connection hooks

| ID | what | status | evidence at HEAD | value to extract |
| --- | --- | --- | --- | --- |
| F171 | `#[on_connect]` / `#[on_disconnect]` / `#[after_init]` forwarded arguments by position; a connect hook could not take `Session`, `Extensions`, a handshake query or a custom extractor; `&WsClient` there, `WsClient` in a message handler | superseded | hooks are trait methods with typed contexts: `OnConnect::on_connect(&self, cx: &ConnectCx)`, `OnDisconnect::on_disconnect(&self, conn: &Connection, why: DisconnectReason)` (`ulo-ws/src/gateway.rs:265-271`); `ConnectCx` exposes `ext()`, `extensions()`, `conn()`, `head()` (`ulo-ws/src/transport.rs:189-207`); a session is a `FromContainer` read seeded for the connect phase (transports DESIGN §4.1, *Sessions*) | — |
| A6 | `DisconnectReason` unreachable from a macro-written `#[on_disconnect]` | built | delivered to every `on_disconnect` (`gateway.rs:271`, `:298-301`); `ulo-ws-conformance/src/cases/hooks.rs` names it | — |

### 3.7 Surface, coverage and the DI-owned pair

| ID | what | status | evidence at HEAD | value to extract |
| --- | --- | --- | --- | --- |
| F172 | The public extraction surface named `multer`, `validator` and `bytes` without re-exporting them | built | `ulo_http::multipart::{Field, Error}`, `ulo_http::Bytes`, `ulo_http::headers` (`ulo-http/src/lib.rs:90-91`, `:129-134`), asserted in `ulo-http/tests/reexports.rs:42-52`; the rule is principle 7 of core DESIGN §0 with `scripts/public_reach.py` in CI; `validator` leaves the public signatures, replaced by `FieldViolation` and an opt-in bridge | — |
| F173 | `Multipart` and `Body<T>` had no integration test | built | `ulo-http/tests/extract.rs:271-457`: one test per body extractor arm, multipart text and file parts, mid-stream 413, missing boundary, malformed parts | — |
| F174 | `Path` dropped serde's error and named the whole parameter map; `PathError::NotFound` was unreachable | built | the shared pair deserializer names the field (`de.rs:63-80`); `tests/extract.rs:254-255`; no `NotFound` variant | — |
| F180 | `ulo::http::Request` wrapped `RequestPart` to add two maps nothing read, and ran a fourth query parser | superseded | the head is an execution input, `Dep<RequestHead>` (transports DESIGN §2.10), and `HttpCx::head()`; `Query` and `Form` share one parser, `parse_form` (`de.rs:31-48`) | The probe result stands as a general fact: ulo's own traits can be implemented for a foreign type such as `http::request::Parts`, so a wrapper is never required to make a foreign type injectable or extractable |
| F181 | `Extension<T>`'s per-type registration built a view over an empty bag | superseded | `Ext<T>` is a `FromContainer` read of the execution's extensions with no registration (`crates/ulo/src/dependency/ext.rs:9-14`; core DESIGN §3.2 table) | — |
| B11 (docs ledger) | `crates/ulo/src/rpc/extractors.rs` documented the removed bare-DTO form | dropped | the file does not exist on this branch | — |

---

## 4. Decisions D1–D7

| ID | what | status | evidence at HEAD | value to extract |
| --- | --- | --- | --- | --- |
| D1 | An extraction failure reaches the error chain carrying its type: `FromContext::Error: ulo::Error`, lifted as `AppError` so `#[catch(JsonError)]` matches | superseded | one concrete `ExtractError` reaches the handlers as a `CallError` holding it as source; a handler reshapes with `err.downcast_ref::<CallError>()?.source_as::<ExtractError>()` (`extract.rs:84-87`) | The seam half is built. The typed-error half is not: an extractor carries no error type of its own (§5, §7 Q4). Rejected alternatives worth keeping, verbatim in substance: (a) *keep the bypass* — refuted because guard refusals, unrouted patterns and panics all reach the chain though none is the application's error, and a service answering `application/problem+json` wants its 400 in that shape; (b) *reach the chain erased* (`bad_request(e.to_string())`) — fixes the seam, leaves the status a literal and the other transports at `Internal` |
| D2 | Markers stay as syntax and lower onto the extractor path; `use_extractors` deleted | dropped | markers removed (`SWITCH_INVENTORY.md:74`, `:303`) | The memo records that the user, 2026-09-22, called the markers "a deliberate alternative syntax rather than legacy" and that deleting them was "not proposed". The rebuild deleted them. §7 Q6 |
| D3 | `#[param("id", default = …)]` refused at expansion; `#[query]` defaults parsed at expansion, missing distinguished from unparseable | superseded | markers gone; `Path<T>` is checked against its route in `prepare` by a recording deserializer (`ulo-http/src/extract/path.rs:75-97`), so a struct field the route does not name is a startup error whatever its serde default | D3's argument is what the startup check enforces: a path parameter is present whenever the route matched, so a default can only apply to a name the route does not declare, which is a typo. `#[serde(default)]` on a `Path<T>` field cannot suppress the check, since the recorder sees every field name |
| D4 | One `Content-Type` policy for both body extractors: `application/json` and every `+json`, absent header unknown-and-try, 415 otherwise | built, with a departure | `body.rs:48`, `:240-243`; absent header 415 (`DIVERGENCES.md:534`) | Rejected alternative kept: *leaving `Json<T>` permissive and giving only `Body<T>` the 415* keeps two body extractors with opposite policies. The rebuild has one policy; the absent-header departure is §7 Q3 |
| D5 | The WebSocket connection hooks read their parameters through `FromContext<WsContext>` | superseded | trait hooks with typed contexts (§3.6) | Rejected alternative: a positional slot for `DisconnectReason` alone would have closed A6 and left `Session`, `Extensions` and custom extractors unreachable. The rebuild reaches all of them through `ConnectCx` and container reads |
| D6 | A single typed header: parked until a header's value needs parsing; implementation named as the `headers` crate behind a feature | built | `Header<H: headers::Header>` (`ulo-http/src/extract/head.rs:12-38`); absent `Missing` naming the header, unparsable `Malformed`; `tests/extract.rs:411-420` | The comparator survey fetched for D6: Nest `@Headers(name?)`; axum `HeaderMap` and `TypedHeader<T>` in `axum-extra`; actix-web `web::Header<T>`; poem `web::Header<T>`. The rebuild took the `headers` crate as D6 named, unconditionally rather than behind a feature |
| D7 | An injectable is shared, not copied: `Arc<T>` for singleton and execution scope (DI's D14) | superseded | owned by the DI subject; `Ext<T>(Arc<T>)` (`ext.rs:14`), `Dep::into_arc` (`emit.rs:207`) | The one-line argument: scopes were read as promises about identity and were promises about construction count; both layers already held the `Arc` and discarded it |

### Proposed ADRs

| ID | what | status | evidence at HEAD | value to extract |
| --- | --- | --- | --- | --- |
| ADR-A | *An extraction failure is an error* (D1, D4; supersedes 0023's *Roads not taken*) | superseded | transports DESIGN §2.2 *Extraction failure has one shape everywhere* and §3.6 *Failure* carry the decision | — |
| ADR-B | *One extraction language on HTTP* (D2, D3, D5) | superseded | transports DESIGN §2.2 and `params.rs:1-7` | — |
| ADR-0023 pointer | One line under 0023's title pointing at 0040 | open | as F178 | — |

---

## 5. Build rows

| ID | what | status | evidence at HEAD | value to extract |
| --- | --- | --- | --- | --- |
| EXT-ADR (row 64) | The two ADRs, `proposed` | superseded | as ADR-A, ADR-B | — |
| EXT-1 (65) | `FromContext::Error: ulo::Error`; eleven `impl Error`; `BodyExtractionError` gains `Error` | superseded | `ExtractError` (`extract.rs:80-183`) | — |
| EXT-2 (66) | The four lift sites carry the error | built | `__private.rs:49-51`; HTTP `tests/extract.rs` reporting app; RPC `errors.rs:39-43` | — |
| EXT-3 (67) | Each extractor error names its kind: `BadRequest` / `UnprocessableEntity` / `Internal` for `BodyAlreadyRead` and `RequestError` | **open** (two of three) | validation 422 built; shape mismatch 400 (§3.1); taken body `BadRequest` (§3.2) | §3.1, §3.2 |
| EXT-4 (68) | One `Content-Type` policy, `+json`, 415 | built, with a departure | §3.3 | — |
| EXT-5 (69) | `Validated` carries `E::Error`; `details()` serialises `ValidationErrors` | built | `validate.rs:54-60`, `extract.rs:176-181` | — |
| EXT-6 (70) | Markers lower onto the extractor path | dropped | markers gone | — |
| EXT-7 (71) | `#[param]` default refused; `#[query]` default parsed at expansion | dropped | markers gone | — |
| EXT-8 (72) | Marker read searches all attributes; per-key branch stops reading primitive names | dropped | markers gone | — |
| EXT-9 (73) | `ExtractorKind` collapses to four; `HttpRequest` arm deleted; `Optional` arm to the blanket | superseded | `Param<T, M>`; `Option<P>` blanket only (`extract.rs:39-53`) | — |
| EXT-10 (74) | A parameter the macro cannot name refused with gRPC's diagnostic in the other three macros | built | better than planned: every pattern is accepted (`emit.rs:195-213`) | — |
| EXT-11 (75) | The context reference diagnosed at the parameter | superseded | the context is a by-value `FromCall` parameter; a `&HttpCx` falls to `Param`'s `on_unimplemented` (`__private.rs:39-41`) | What that diagnostic reads for `&HttpCx` is UNVERIFIED (§9) |
| EXT-12 (100) | WS hooks through `FromContext<WsContext>`; `DisconnectReason` an extractor | superseded | §3.6 | — |
| EXT-13 (24) | `multer`, `validator`, `bytes` re-exported | built | merged on master as #362; rebuilt independently (§3.7 F172) | — |
| EXT-14 (107) | `Path` reports its error; `PathError::NotFound` removed | built | §3.7 F174 | — |
| EXT-15 (25) | One `Multipart` round trip and one test per `parse_bytes` arm | built | merged on master as #363; the rebuild's `tests/extract.rs:302-457` | — |
| EXT-16 | ADR-0023 gains its pointer to ADR-0040 | open | as F178 | — |

**The custom-extractor question raised by EXT-1's replacement (value to extract).** D1 chose the
associated-error route so that an application's own extractor failure would travel with its own
type and kind. At HEAD `FromCall::from_call` answers `Result<Self, ExtractError>`
(`extract.rs:32`). A custom extractor can construct `Missing`, `Malformed` (whose `source` needs a
`Redacted`, obtained through `AppHandle::redact`), `UnsupportedMediaType`, `TooLarge`, `Invalid`
or `HostMissing`; none classifies as `Unauthorized`, `Forbidden`, `NotFound` or `Conflict`. The
only route to another kind is `Dependency`, which wraps a container `LookupError` and classifies
through it (`extract.rs:100-103`, `:162`): an execution-scoped constructor returning a
`CallError::unauthorized(..)` reaches the caller as 401 when read as `Dep<CurrentUser>`. So the
rebuild's answer to "an extractor that authenticates" is a `FromContainer` type, a pre-dispatch
entry or a guard, not a `FromCall` impl. That answer is defensible; it is not written down. §7 Q4.

---

## 6. Method, memo-local findings and owed verifications

| ID | what | status | evidence at HEAD | value to extract |
| --- | --- | --- | --- | --- |
| M1 | A batched spelling audit hides its silent cases: a resolution error aborts the crate before the type checker reaches `const` assertions and `E0277` cells | **open** (as a test gap) | the rebuild's two compile-time claims (body pairs, alias equivalence) are exercised only under `docs/experiments/di-redesign/probes/src/bin/p25_body_consumer_pairwise.rs` and `p25b_body_consumer_two_fails.rs`; `crates/ulo-codegen-tests/tests/ui/` has no fixture containing "both consume the body" and no accepted-spelling suite exists. Ledger: F302 states the general absence of tests; this case is not named | See below |
| M2 | The RFC 9110 status table (§2 of the memo) | built except one row | §3.1 | — |
| M3 | Extraction runs innermost, after guards and the interceptor's before-phase | built | `emit.rs:41` | — |
| M4 | `Request` was the only type both an injectable and an extractor; worth keeping as a shape | superseded | `RequestHead` is an input read by `Dep` and by `HttpCx::head()` | — |
| M5 | The four context reads were one defect whose diagnostic named a type the author did not write (`&HttpContext is not an extractor for HttpContext`) | superseded | as F166 | The diagnostic lesson: a refusal must name what was written, not what it resolved to. R12's falsification showed `on_unimplemented` does that; the rebuild relies on it (`Param`, `FromCall`) |
| M6 | `ExtractorKind::Optional` duplicated the blanket `Option<T>` impl | superseded | one blanket, narrowed to `Missing` and `HostMissing` (`extract.rs:35-53`), where the old one swallowed every error | The narrowing is an improvement and is also what makes F164's `None` possible |
| M7 | Out-of-scope pointers: F117, F111, F112/F116, F136, ADR-0053, RPC request-side streaming | not this subject's | F111: one WHATWG decode (`de.rs:31-48`); F112/F116: `body_limit` with 413 (`body.rs:217-273`); F136: exact status on kind `BadRequest` (`render.rs:59-62`); ADR-0053: RFC 9457 bodies (DESIGN §2.4); RPC `Inbound<T>` exists (`ulo-rpc/src/extract.rs:78`); F117: a repeated key on a struct field is `Malformed` (`DIVERGENCES.md:524`), reachable only as `Query<Vec<(String, String)>>` | Listed so the HTTP reader can confirm; not graded here |
| S1 | The query-parsing crate had no owner; decided 2026-09-24 as `serde_html_form` | superseded | the rebuild wrote its own pair deserializer (`ulo-http/src/extract/de.rs`) and depends on no form crate | The registry facts recorded with S1: `serde_html_form` 0.4.1, MSRV 1.63, a `serde_urlencoded` fork with sequence fields from a repeated key, built on `form_urlencoded`. Relevant if F117 is reopened: the rebuild's deserializer refuses a repeated key on a struct field, which `serde_html_form` reads as a `Vec` |
| V2 | A global error handler claims an extraction failure on all four transports; removing it restores the canonical rendering | built on HTTP, **open** elsewhere | HTTP: `tests/extract.rs:191-235`. RPC conformance asserts the rendering (`errors.rs:39-43`) but no test on RPC, WS or gRPC has an error handler claim an `ExtractError` (no `source_as::<ExtractError>` outside `ulo-http`) | The test as specified in the build handoff, per transport |
| V3 | A status table over the failure classes on every HTTP adapter, including a guard-consumed body answering 500 | built except the last case | `ulo-http-conformance/src/cases/extraction.rs:9-27` | The guard-consumed case is F164's test |
| V4 | Six media types against both body extractors | partial, **open** | `tests/extract.rs:275-282` lacks `application/problem+json` and no header | — |
| V6 | One handler mixing a marker, `Json`, a custom extractor and the context | dropped | markers gone | — |
| V7 | `?n=abc` against a defaulted marker; a bad default a compile error | dropped | markers gone | — |
| V9 / V11 | The §4 matrix as compile-fail fixtures, each diagnostic naming what was written | **open** | as M1 | See below |
| V12 | A macro-written `on_disconnect` reads `DisconnectReason`, `on_connect` reads `Session` | superseded | hooks are trait methods (§3.6) | — |

**M1 / V9: value to extract.** The audit's spelling matrix (memo §4), at `654c7183`, one crate
build per cell. Accepted, in one batch: `Json<Dto>` fully qualified, through a renamed import,
behind a `type` alias; `Bytes` and `HttpRequest` through renamed imports and aliases;
`Option<Json<Dto>>` written `std::option::Option<…>` and behind an alias; a custom `CONSUMES`
extractor; two custom parts-only extractors; one beside `Json`; `Validated<Query<T>>` beside
`Json<U>`; five parts-only extractors and the context on one handler. Refused: R3 `Json` beside
`Bytes` and R4 `HttpRequest` beside `Json` (`E0080`, both names, the rule working); R10
`_: Json<Dto>` and R11 `Point { x, y }: Point` (`E0061`, the dropped-parameter defect); R12
`n: u32` (`E0277` with the `on_unimplemented` note, the falsification); R1/R2/R13–R15 context refs
through an alias or rename; R5–R9 marker mixes. The method rule: **each cell compiles in its own
crate**, because "fourteen of the cases in one file reported two refusals and twelve silences …
all twelve of the silent cells were refusals". trybuild builds each `tests/ui/*.rs` as its own
program, which satisfies the rule; a single file of `const` assertions does not. The rebuilt
equivalents to add, each its own case: accepted — `type B<T> = Json<T>`, `use ulo_http::Json as J`,
`ulo_http::extract::Json<T>` qualified, `std::option::Option<Json<T>>`, `_: Json<T>`,
`Point { x, y }: Path<Point>`, `Valid<Query<T>>` beside `Json<U>`; refused — `Json<T>` beside
`Form<U>`, `type B<T> = Json<T>` beside `Bytes` (the alias must still count), `&HttpCx`, `n: u32`.

---

## 7. Worth discussing with the designer

1. **Should a JSON (or form, query, path) value that parses and does not fit `T` answer 422?**
   RFC 9110 §15.5.21 describes exactly that case; `serde_json` distinguishes it through
   `Error::classify()`. Recommendation: yes for `Json<T>`, mapping `Category::Data` to a 422 kind
   with one `FieldViolation` where serde names the field; leave `Form`, `Query` and `Path` at 400,
   since their values arrive as text and a parse failure there reads as malformed input.
2. **Is a body already taken by a guard the client's fault?** At HEAD it is `Missing`, 400, and
   `Option<Json<T>>` reads it as `None`. Recommendation: no. Give "taken" its own cause that
   classifies `Internal` with the message withheld, log it at `error`, and keep `Option<P>` from
   swallowing it. gRPC's old `RequestError` rule is the precedent for the same condition.
3. **No `Content-Type` on a JSON body: 415 or try?** D4 chose try; the rebuild chose 415 and
   recorded it (`DIVERGENCES.md:534`). Both readings are RFC 9110 §8.3's. Recommendation: keep 415,
   the stricter of the two, and add the absent-header and `application/problem+json` cases to
   `tests/extract.rs` so the choice is pinned rather than incidental. Judgment call.
4. **A custom `FromCall` extractor cannot name a kind beyond 400/422/500.** Recommendation: state in
   the `FromCall` documentation that an extractor whose failure means 401, 403 or 404 is a
   `FromContainer` type with an execution-scoped constructor, a guard, or a pre-dispatch entry, and
   name `Dependency` as the variant that carries such a failure. An `ExtractError::Call(CallError)`
   variant is the alternative if that answer is not wanted.
5. **ADR-0023 and the `docs/adr/` series.** Every extraction ADR on this branch describes the
   replaced framework. Recommendation: decide at the switch whether `docs/adr/` is superseded as a
   series by the DESIGN documents; if kept, one superseding record for extraction replaces both the
   0023 pointer (F178) and the two proposed ADRs.
6. **The markers.** The audit records the user calling `#[body]` / `#[param]` / `#[query]` a
   deliberate alternative syntax (memo §9, D2); the rebuild dropped them
   (`SWITCH_INVENTORY.md:303`). Recommendation: confirm the drop with the user explicitly, since it
   reverses a recorded position. Nothing in the rebuild needs them: `Path`, `Query` and `Json`
   cover every case the markers did, without the name lists that produced F169, F170 and F179.

---

## 8. New observations at HEAD

| ID | what | status | evidence | value to extract |
| --- | --- | --- | --- | --- |
| N1 | A `#[cfg(..)]` written on a handler parameter: `params.rs` reads no parameter attributes, and `emit.rs:197-213` emits an extraction and an argument for every parameter. Where the predicate fails, rustc removes the parameter from the method and the generated call would pass one argument too many | UNVERIFIED | `crates/ulo-handler-codegen/src/params.rs:65-77` (attributes ignored); transports DESIGN §2.1 lines 83-87 treats `cfg` on handlers and their attributes, not on parameters | Needs one compile: a handler with `#[cfg(any())] x: Json<T>`. If it fails with an arity error, the attribute should either refuse a `cfg` on a parameter or gate the extraction and argument the way §2.1 gates mount calls |

---

## 9. Not verifiable here

These need a build or a run; only N1 is counted as an item, the other two qualify rows already
graded (EXT-11 superseded, V2 open).

- **N1** above.
- **The diagnostic for `&HttpCx` as a parameter** (EXT-11). `FromCall: 'static` refuses a
  reference; whether the message reads `` `&HttpCx` cannot be built from a `Http` call `` through
  `Param`'s `on_unimplemented` (`__private.rs:39-41`), or a lifetime error first, needs a compile.
  The memo's R1 lesson is that the message should name what the author wrote.
- **Dispatch of an extraction failure to a claiming error handler on WS and gRPC** (V2): the
  classification and rendering are read (`extract.rs:158-183`, `status.rs:23`) and the RPC
  rendering is tested; the claim path on the three non-HTTP transports needs a run.
