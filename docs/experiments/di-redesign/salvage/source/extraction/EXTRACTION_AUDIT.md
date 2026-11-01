# Extraction audit — what a handler may ask for, and what happens when it cannot have it

Opened 2026-09-22. Scope set by `EXTRACTION_AUDIT_HANDOFF.md`: `FromContext` and everything built on
it — the HTTP extractors, the transport-agnostic ones, gRPC's `Inbound` and `GrpcRequest`, the
`#[body]` / `#[param]` / `#[query]` markers, the one-body rule, and what a failed extraction answers
on each of the four transports. One cross-transport surface, the way the pipeline was.

**Where the findings live.** `FRAMEWORK_GAPS.md`, F163–F181, plus a correction appended to F84 and a
third instance appended to F1. F180 and F181 are DI surfaces this pass reached and they are answered
by `DI_AUDIT.md`'s D14, which this pass raised. This memo is the plan and does not restate them.

**Not a build.** Nothing is committed and no branch exists. Five probe files were added under
`integration-tests/tests/integration/` and removed, and one existing fixture was edited for one
compile and reverted; `git status` is clean at `654c7183`.

## Confidence marking

- **Probed** — run this session against master `654c7183` through the real macro and dispatch path.
  Each compile-time probe carries its falsification: the same spelling in the form that is expected
  to be accepted, compiled in the same session. The spelling matrix in §4 compiles every cell, one
  crate build per cell, because a resolution error in one case aborts the crate before the type
  checker reaches any other — a batched run reported two refusals and hid twelve unrun cases.
- **Read** — read off the source, cited by file and line in the ledger entry.
- **Measured** — a count, with what was counted stated.
- **Spec** — RFC 9110 and RFC 6838, fetched this session and quoted rather than paraphrased.
- **Unverified** — believed, not established. Marked inline, and not acted on before falsification.
  **Nothing below carries this tag.** Two claims were written as Read and upgraded before the memo
  was finished: gRPC's context read, which was going to be inferred from three identical functions
  and was compiled instead, and the pipeline position of an extraction failure, which F84 states
  wrongly and which a recording guard and interceptor settled.

Under the three-category test in `project-the-spec-conformance-seam`, extraction is category (3):
no published standard mentions an extractor, a `FromContext` impl or a one-body rule. **The
category-(1) edge the handoff predicted is real and larger than it expected.** It is not the failure
*body* — that is ADR-0053's — but the failure *status*: RFC 9110 names a status for each of the
three conditions the extractors detect, and the framework answers 400 for all three. §2 is that.

## The shape of it

Two independent problems, and a field of consequences. **The first is that a failure's fate is
decided by which transport ran it** — §1 and §2, the same defect seen from the chain and from the
wire, and under both of them one fact: `FromContext::Error` is bounded by `Display`, so an extraction
failure has no type to carry past its own `to_string()`. **The second is that HTTP has two extraction
languages and a handler may use only one of them** — §3, with §4's matrix as its evidence. §5 is what
that matrix turned up on the way and belongs to neither: it is a parameter-pattern defect in three of
the four handler macros, which the fourth already diagnoses. §6 and §7 are surface questions and can
be taken in any order. §8 is the residue, six paragraphs that change no design. §9 records the
decisions, F84 first; everything above it is the evidence for them.

---

## 1. A failure reaches the chain on three transports and the right status on one

**(Probed on three, Read on gRPC.)** ADR-0035 made the error chain the one error seam. An extraction
failure reaches it on RPC, WebSocket and gRPC, and does not on HTTP. The three that reach it render
the caller's own bad input as a server fault.

| transport | what the wrapper does with the failure | reaches `#[catch]` | what the caller gets |
| --- | --- | --- | --- |
| HTTP | `ExecutionResult::Ok(HttpResponse { status: 400 })` | no | `400 {"error":"Extraction failed","details":…}` |
| RPC | `ExecutionResult::Err(RpcError::Internal(e.to_string()))` | yes, as `RpcError` | `{"status":"error","kind":"Internal"}` |
| WebSocket | `ExecutionResult::Err(WsError::Internal(e.to_string()))` | yes, as `WsError` | the canonical envelope, kind `Internal` |
| gRPC | `Err(GrpcStatus::internal(e.to_string()))` | yes, as `GrpcStatus` | `INTERNAL` |

The probe registered a global error handler that claims everything and sent a payload that cannot
deserialise. RPC and WebSocket both answered `claimed-by-chain` with the handler's counter at 1, and
the text it was handed was `Internal error: invalid type: string "not-a-number", expected u32`. HTTP
is pinned the other way by `extraction_failure_shape.rs`, which asserts that the same handler does
not claim, and §1's own order probe shows the 400 arriving at the interceptor as a success.
**gRPC's row is Read**, off `lower_handler`'s lift (`crates/ulo-macros/src/grpc_macro/grpc_methods.rs:702-713`)
and the seam in `crates/ulo/src/grpc/runtime.rs`; the chain-reachability half of it was not run.

**Each transport has one half of the answer and none has both.** HTTP names the fault correctly
and puts it outside the seam; the other three are inside the seam and name the fault wrongly. F84 is
the first half, F1 the second; gRPC is a third instance of F1 that its entry does not name, and is
appended to it.

**F84's entry is wrong about where the bypass happens, and the correction matters to the fix.** It
says the 400 is built "before any guard, interceptor or chain handler runs". Guards and interceptors
both run first: extraction is inside `Route::execute`, which is `RouteLeaf::call`, which is the
innermost step of `through_interceptors` (`crates/ulo/src/http/pipeline.rs:19-35`, `:165-176`).
Probed with a recording guard and a recording interceptor over a handler whose `Json<T>` cannot
deserialise, the sequence is `["guard", "interceptor:before", "interceptor:after:Ok(400)"]` — the
interceptor's after-phase is handed the extraction failure as a success. Only the chain is bypassed,
and it is bypassed because the failure leaves as `Ok` rather than because of where it is raised.

**Under all four rows is one bound.** `FromContext::Error: fmt::Display`
(`crates/ulo/src/extract/from_context.rs:32`). `Display` is all a wrapper can use, so every one of
the four does the only thing available: `e.to_string()`. The chain borrows
`&dyn Error + Send + Sync + 'static` and the renderer reads `Error::kind()`; an extraction failure
can satisfy neither, so no transport could do better than it does. Every extractor error in the tree
already implements `std::error::Error` — with one exception, `BodyExtractionError<E>`, which is the
error type of `Json`, `Body`, `Bytes`, `BodyStream` and `Multipart` and implements only `Display`
while every error it wraps implements both. F165.

## 2. The status is not the one the specification names, for two of the three failure classes

**(Spec, quoted from RFC 9110 fetched this session; the behaviour Probed.)** The framework detects
three distinct conditions in a request body and answers 400 for all of them. RFC 9110 §15.5 names a
different status for two.

> **§15.5.1.  400 Bad Request.** The 400 (Bad Request) status code indicates that the server cannot
> or will not process the request due to something that is perceived to be a client error (e.g.,
> malformed request syntax, invalid request message framing, or deceptive request routing).

> **§15.5.16.  415 Unsupported Media Type.** The 415 (Unsupported Media Type) status code indicates
> that the origin server is refusing to service the request because the content is in a format not
> supported by this method on the target resource. The format problem might be due to the request's
> indicated Content-Type or Content-Encoding, or as a result of inspecting the data directly.

> **§15.5.21.  422 Unprocessable Content.** The 422 (Unprocessable Content) status code indicates
> that the server understands the content type of the request content (hence a 415 (Unsupported
> Media Type) status code is inappropriate), and the syntax of the request content is correct, but
> it was unable to process the contained instructions.

Against those three, each row sent through a live server:

| what the request carried | ulo answers | RFC 9110 |
| --- | --- | --- |
| `Content-Type: application/json`, body `<<<not json>>>` | `400 {"details":"Failed to deserialize JSON body: expected value at line 1 column 1"}` | 400 |
| `Content-Type: text/plain`, body `{"name":"ada"}`, through `Body<T>` | `400 {"details":"Unsupported content type: text/plain"}` | **415** |
| `Content-Type: application/json`, body `{"wrong":1}` | `400 {"details":"Failed to deserialize JSON body: missing field \`name\`…"}` | **422** |
| a body that fits `T` and fails `#[validate(length(min = 3))]` | `400 {"details":"Validation failed: name: Validation error: length […]"}` | **422** |

`ErrorKind::UnprocessableEntity` exists and maps to 422 (`crates/ulo/src/http/error.rs:42`), so the
third and fourth rows are answerable today and are not answered. The string `415` appears nowhere in
core or in any of the five adapters.

**415 is a status rather than a kind, which is why it does not want an `ErrorKind` variant.** A media
type is HTTP's; RPC, WebSocket and gRPC have nothing to say about one. The pipeline audit's F136
settles the shape this rides on — `Status { kind, message }` beside `Custom { status, message }` on
each transport's error — and 415 is what the second variant is for.

**Neither correction is reachable while §1 stands.** The status is written as the literal `400` in
the macro, nine times (`extracts_marker_params.rs`, eight; `extractor_params.rs`, one), and the
extractor's own error never reaches anything that could read a status off it. So §2 is a consequence
of §1's decision rather than a separate build.

**A fifth condition is detected and then not used.** `Body<T>` reads `Content-Type` and refuses what
it does not know; `Json<T>` never reads it at all (`crates/ulo/src/http/extract/json.rs:75-86`).
Probed with the same bytes and the same header on two handlers: `Json<T>` answers `200 json:ada`
where `Body<T>` answers `400 Unsupported content type: text/plain`. Two body
extractors, opposite policies, neither stated as one. `Body<T>`'s test is `contains("application/json")`,
which also refuses every `+json` media type — RFC 6838 §4.2.8 registers the suffix convention that
makes `application/problem+json` and `application/vnd.api+json` JSON — and `application/problem+json`
is the type ADR-0053 gives ulo's own error body. F176.

## 3. HTTP has two extraction languages and a handler may use only one

**(Read, with §4's matrix compiling each case.)** A parameter is either a typed extractor or a
marked one, and which of the two a handler is written in picks one of two code paths for the whole
signature: `use_extractors = has_extractors || marker_params.is_empty()`
(`crates/ulo-macros/src/controller_macro/instance_injection.rs:220`). Each path binds only its own
parameters, so a signature that mixes them leaves the other kind unbound. A16 records that; what
follows is what decides *which* failure the author gets, and the three bugs the second path carries
that the first does not.

**The switch reads a hardcoded name list.** `has_extractors` is *any parameter whose kind is neither
`HttpRequest` nor `Unknown`* (`:217-219`), and the kinds come from `detect_extractor_kind`, which
matches the last path segment of the written type against ten names
(`controller_macro/extractor_params.rs:232-244`). So whether a marker beside a second parameter
produces one failure or another depends on whether that parameter's *name* is on the list:

| beside a `#[query]` marker | kind | path taken | what the author is told |
| --- | --- | --- | --- |
| `Json<Dto>` | `Json` | extractor | the marker is unbound: `E0061` on their own method |
| a custom extractor | `Unknown` | marker | the extractor is unbound: `E0425` on a name they wrote |
| `HttpRequest` | `HttpRequest` | marker | `E0425: cannot find value __req` — an identifier nothing binds (A17) |
| `&HttpContext` | `Context` | extractor | the marker is unbound: `E0061` |

One mistake, four routes, and none of the diagnostics mentions markers. The `HttpRequest` row is the
sharpest: `generate_method_call` emits `__req` for it (`instance_injection.rs:293`) and nothing in
`generate_route_wrapper` binds that name — only `_req_parts` (`:392`) — so the branch has never
compiled for anyone.

**The second path carries three bugs the first does not, all three in its defaults, each sent through
a live server.**

- `#[param("id", default = "9")]` parses its default and never reads it
  (`markers_params/get_marker_params.rs:52-56` against `extracts_marker_params.rs:182-224`). Probed
  in the only case where the default could apply — a marker naming a parameter the route does not
  declare — and the answer is `400 {"error":"Missing required path parameter","param":"missing"}`. A2.
  **That case is a typo and nothing else**, because a path parameter is present whenever the route
  matched, which is the argument for refusing the argument rather than implementing it.
- `#[query("n", default = "abc")] n: usize` parses the literal per request and `expect`s it
  (`extracts_marker_params.rs:88`). Probed: `500 {"message":"panic recovered in handler: Invalid
  default value: ParseIntError { kind: InvalidDigit }"}` — a per-request 500 carrying the panic's own
  text to the client. A3.
- `#[query("n", default = "1")] n: usize` treats an unparseable value and a missing one alike —
  `.and_then(|v| v.parse().ok()).unwrap_or_else(…)` (`:87-88`). Probed: `?n=abc` answers
  `200 marker-query-default:1`, where the same marker without a default answers
  `400 {"error":"Failed to parse query parameter","param":"n"}`. **Adding a default turns a rejected
  input into an accepted one.** C4.

**And the two paths speak different error JSON.** The typed path writes one shape from one function:
`{"error":"Extraction failed","details":…}` (`extractor_params.rs:324-336`). The marker path writes
six more strings across eight sites — *Failed to extract request body*, *Failed to extract query
parameters*, *Failed to parse query parameter*, *Missing required query parameter*, *Failed to parse
path parameter*, *Missing required path parameter* — three of them carrying a `param` field the typed
path has no equivalent of. Nine hardcoded `status: 400` blocks in the macro crate between them. C3,
measured. The same bad path parameter through the two paths, probed side by side:

```
Path<i32>          400 {"error":"Extraction failed",
                        "details":"Failed to parse path parameter: Failed to
                                   deserialize path params from {\"id\": \"abc\"}"}
#[param("id")] i32 400 {"error":"Failed to parse path parameter", "param":"id",
                        "details":"Parse error: invalid digit found in string"}
```

The marker path's message is the better of the two: it names the parameter and says what was wrong
with it, where the typed path names the whole map (F174).

**A15 is closed, and closing it put C2 back.** `#[query("name")]` on a non-scalar type used to emit
`::ulo::Query`, a path that did not resolve, which is why C2 — *the name is ignored* — was recorded
as dissolved into a bug that had never compiled. The module reorganisation corrected the path to
`::ulo::http::extract::Query` (`extracts_marker_params.rs:161-162`), so the branch compiles now and
does what C2 said: the marker's argument is dropped and the whole query string is deserialised into
the parameter's type. **C2 is live, unfiled, and its premise has changed** — it is no longer a
never-compiled branch. F169.

**`is_scalar_type` is a second name list, and it decides the mechanism rather than the diagnostic.**
It matches the written type's last segment against fifteen primitive names plus `Option`
(`extracts_marker_params.rs:227-260`). A parameter written `usize` takes the per-key path; the same
parameter written through `type Count = usize;` is not on the list, so it takes the `Query<Count>`
path and deserialises the entire query string into a `usize`. Probed: `?n=5` answers
`400 {"details":"Failed to deserialize query parameters: invalid type: map, expected usize"}`.
**A handler written that way cannot answer any request**, and it compiles. F170.

## 4. The spelling matrix

**(Probed. Fifteen cells, one crate build each.)** The method is
`feedback-the-spelling-audit`: write the same type behind an alias, through a renamed import, fully
qualified and nested, compile each, and read both outputs — which spellings are refused, and whether
the message names the author's own code or the framework's.

**The batched run is the caution this pass adds to that method.** Fourteen of the cases in one file
reported two refusals and twelve silences. A resolution error aborts the crate before the type
checker runs, so the `const _: () = assert!(…)` cells and every `E0277` cell were never evaluated —
and the run exited reporting errors, which looks like a run that worked. Each cell was then given its
own build, and **all twelve of the silent cells were refusals**. A batched spelling audit reports the
two loudest cases and calls the rest clean.

### What is accepted

Compiled as one batch, exit 0. Every one of these is a spelling the macro does not recognise and
that works anyway, because the `_` arm sends it through `FromContext` like any other type:

`ulo::http::extract::Json<Dto>` fully qualified · `Json` through a renamed import · `Json<Dto>`
behind a `type` alias · `Bytes` through a renamed import · `HttpRequest` through a renamed import ·
`HttpRequest` behind an alias · `Option<Json<Dto>>` written `std::option::Option<…>` ·
`Option<Json<Dto>>` behind an alias · a custom extractor declaring `CONSUMES` · two custom
parts-only extractors on one handler · a custom parts-only extractor beside `Json<Dto>` ·
`Validated<Query<T>>` beside `Json<U>` · five parts-only extractors and `&HttpContext` on one
handler.

### What is refused, and what the author is told

| # | written | outcome | diagnostic |
| --- | --- | --- | --- |
| R1 | `&HttpContext` through a renamed import | refused | `E0277: &HttpContext is not an extractor for HttpContext` |
| R2 | `&HttpContext` behind a `type` alias | refused | the same |
| R3 | `Json<Dto>` beside `Bytes` | refused | `E0080: \`_j\` and \`_b\` both read the request body` |
| R4 | `HttpRequest` beside `Json<Dto>` | refused | `E0080`, both parameters named |
| R5 | `#[query("q")] q: String` beside `Json<Dto>` | refused | `E0061: this method takes 2 arguments but 1 argument was supplied` |
| R6 | `#[query("q")] q: String` beside a custom extractor | refused | `E0425: cannot find value \`p\`` |
| R7 | `#[query("q")] q: String` beside `&HttpContext` | refused | `E0061` |
| R8 | `#[query("q")] q: String` beside `HttpRequest` | refused | `E0425: cannot find value \`__req\`` |
| R9 | `#[allow(unused)] #[query("q")] q: String` | refused | `E0277: String is not an extractor for HttpContext` |
| R10 | `_: Json<Dto>` | refused | `E0061: takes 1 argument but 0 arguments were supplied` |
| R11 | `Point { x, y }: Point` | refused | `E0061`, the same |
| R12 | `n: u32` | refused | `E0277: u32 is not an extractor for HttpContext`, with the `on_unimplemented` note |
| R13 | `&RpcContext` through a renamed import | refused | `E0277: &RpcContext is not an extractor for RpcContext` |
| R14 | `&WsContext` through a renamed import | refused | `E0277: &WsContext is not an extractor for WsContext` |
| R15 | `&GrpcContext` through a renamed import | refused | `E0277: &GrpcContext is not an extractor for GrpcContext` |

### What the matrix says

**All four context reads are the same defect, and the diagnostic reads as a contradiction.**
`detect_extractor_kind`'s reference branch, `is_rpc_context_ref`, `is_ws_context_ref` and
`is_grpc_context_ref` each match the last path segment against a literal
(`extractor_params.rs:201-212`, `patterns_attr.rs:405-412`, `subscriptions_attr.rs:187-194`,
`grpc_macro/grpc_methods.rs:648-656`); a renamed import or an alias is not that literal, so the
parameter takes the extraction path and fails against a `FromContext` impl that does not exist for a
reference. The message names the *resolved* type, so the author who wrote `&Ctx` is told
`&HttpContext is not an extractor for HttpContext` — a sentence about a type they did not write,
which is true of the reference and reads as self-contradictory. F166 carries the class; the context
reads are its fourth instance and are not what ADR-0040 governs, because no downstream check catches
the lie: the alias resolves to the right type and the failure is the macro's routing, not the type's.

**R12 is the falsification.** A type with no `FromContext` impl fails with the `on_unimplemented`
note written for exactly that case, naming the author's type and telling them what to take instead.
So the trait-bound mechanism produces the message the name-list mechanism cannot, on the same
surface, in the same macro.

**R9 is new and unfiled.** Both readers take the parameter's *first* attribute and ask whether it is
a marker — `pat_type.attrs.first()` (`extractor_params.rs:99-103`) and `pat_type.attrs[0]`
(`get_marker_params.rs:18-24`). An `#[allow]`, a `#[cfg_attr]` or any other outer attribute written
before the marker therefore makes the parameter stop being a marker, and it is then extracted as its
own type: `String`, which has no impl. The author is told `String` is not an extractor and nothing
mentions the marker they wrote. F179.

**R3 and R4 are the rule working.** The one-body assertion fires on the pair, names both parameters,
and reads `CONSUMES` off the written type rather than off a list — including through
`HttpRequest`, whose own generated arm bypasses the impl the assertion reads. ADR-0040's mechanism
does what its record claims, which is the thing ADR-0023 said could not be done (§8).

## 5. A parameter the macro cannot name is dropped from the signature

**(Probed, two spellings, each compiled on its own.)** Every handler macro binds a parameter under a
name taken from its pattern, and `extract_param_name` reads only two pattern shapes: an identifier,
and a tuple struct whose first element resolves to one
(`crates/ulo-macros/src/controller_macro/extractor_params.rs:73-85`). Every other pattern answers
`None`, and each of the four macros then `continue`s past the parameter
(`extractor_params.rs:116-119`, `rpc_macro/patterns_attr.rs:374-377`,
`gateway_macro/subscriptions_attr.rs:157-161`; `grpc_macro/grpc_methods.rs:698-701` is the exception
and raises *"a gRPC handler's parameters are named"*).

Dropped means dropped from the generated call as well, so the author is told their own method takes
more arguments than were supplied:

| written | compiles | what the author is told |
| --- | --- | --- |
| `_: Json<Dto>` | no | `E0061: this method takes 1 argument but 0 arguments were supplied` |
| `Point { x, y }: Point` | no | `E0061: this method takes 1 argument but 0 arguments were supplied` |
| `Json(dto): Json<Dto>` | yes | — |
| `dto: Json<Dto>` | yes | — |

`_: Json<Dto>` is how Rust spells an unused parameter, and a struct pattern is how an extractor with
named fields is destructured at the signature. Both are ordinary Rust, both are refused, and the
refusal names neither the pattern nor the macro. Three of the four macros drop the parameter without a word; the fourth
already carries the diagnostic the other three want. F167.

## 6. What a handler may declare differs by transport, and by surface within a transport

F54 surveyed this across the four transports and the user set the criterion for deciding it on
2026-09-14: an extractor exists to reduce boilerplate, the context is available to every handler
anyway, so the question is how much a typed declaration saves over `ctx.headers()`. Two things this
pass adds to that survey, neither of which is about saving boilerplate.

**Headers are declarable, and only through a whole-request view.** `Request` is a
`FromContext<HttpContext>` (`crates/ulo/src/http/provider.rs:136-142`), so
`fn h(&self, req: Request, dto: Json<D>)` compiles and runs — a handler can name its header
dependency at the signature. What HTTP has no extractor for is *one named header*: the three ways to
reach a single one are `Request::header(k)` (`:111-113`), `ctx.request().headers`, and `HttpRequest`,
which consumes the body to hand them over. `Path`, `Query` and the body each got a typed extractor
and the header did not. So the gap is narrower than F54's survey reads: not *headers cannot be
declared*, but *a single typed header has no spelling*. §9's D6 decides what to do about that.

**`Request` is the framework's only type that is both an injectable provider and an extractor**, and
that is the part worth keeping. What is not worth keeping is the rest of it. It carries `method`,
`uri`, `header`, `headers`, `extensions`, `inner`, and two precomputed maps — `path_params` and
`query_params` — which `from_parts` fills on every construction, parsing the query string with
`serde_urlencoded` and swallowing a failure into an empty map (`:82-101`). That is a **fourth** query
parser beside `Query<T>`, the `#[query]` marker's `Query<HashMap<_, _>>` and `Path`'s round trip, and
the four disagree. Measured: `path_params()` has no callers in the workspace and `query_params()` has
one, in a test.

**And the wrapper is not required by the language.** Probed at `654c7183`: `impl
FromContext<HttpContext>` over `RequestPart` compiles and serves a live request (`200 OK GET probe`),
`impl Provider` with `token_of::<RequestPart>()` compiles, and `RequestPart: Send + Sync + 'static`
holds as a `const` assert — `FromContext` and `Provider` are ulo's own traits, so implementing them
for a foreign type is allowed. What the wrapper adds beyond the two maps is an `Arc` and the
accessor sugar; the `Arc` stops being its job under DI's **D14**, and the sugar is a trait. F180 is
the entry; PRs 21 and 22 of the DI build are the work.

**A WebSocket connection hook is the one handler surface in the framework that is not extraction.**
`#[on_connect]`, `#[on_disconnect]` and `#[after_init]` forward their arguments **positionally**,
reading no type at all: the macro counts the typed parameters and emits `self.f()`, `self.f(client)`
or `self.f(client, context)` (`crates/ulo-macros/src/gateway_macro/connection_hook_attr.rs:78-83`).
So a connect hook cannot take `Session`, `Extensions`, a handshake `Query<T>` or any custom
extractor, and the same concept is spelled `&WsClient` in a connect hook and `WsClient` in a message
handler on the same gateway. A wrong type is reported as a mismatched-types error on the generated
call rather than on the parameter.

**A6 is a consequence of that, not a separate defect.** `DisconnectReason` is unreachable from a
macro-written `#[on_disconnect]` because the positional scheme has three slots and no room for a
fourth thing. Were the hooks' parameters `FromContext<WsContext>`, `DisconnectReason` would be an
extractor and A6 would close with nothing written for it. F171.

## 7. The public extraction surface names three crates the root does not re-export

**(Read.)** `crates/ulo/src/lib.rs:47-50` states the policy on the re-export it makes: *"macro-generated
code builds error bodies with it, and `Body::json` traffics in its `Value` type, so consumers need it
in scope without declaring their own dependency."* `serde_json`, `futures`, `async_trait` and
`rustc_hash` are re-exported on that rule. The extraction surface names three crates that are not.

| public item | names | re-exported |
| --- | --- | --- |
| `ulo::http::extract::Field` | `multer::Field` (re-exported *as* ulo's own name) | no |
| `MultipartError::Parse(_)` | `multer::Error` | no |
| `Multipart::next_field` | `multer::Field<'static>` | no |
| `ValidationError::ValidationFailed(_)` | `validator::ValidationErrors` | no |
| `ValidatableExtractor::Inner: Validate` | `validator::Validate` | no |
| `BodyStream::into_stream`, `BodyStream::collect` | `bytes::Bytes` | no |

So an application matching on `MultipartError::Parse(e)` adds `multer = "3"` to its own manifest and
relies on the versions unifying, and one implementing `ValidatableExtractor` for its own extractor
names `validator::Validate` the same way. The framework's own test crate already declares two of the
three (`integration-tests/Cargo.toml` carries `bytes` and `validator`). F172.

**`Multipart` is also the one extractor with no integration test.** `Body<T>` is the other:
`grep` finds no handler in `integration-tests/` taking either, and `Multipart` appears only in
`examples/file_upload.rs`. `Body<T>`'s code path is reached through the `#[body]` marker, which
lowers onto it, so its JSON arm runs — its `UnsupportedContentType` and urlencoded arms do not. F173.

## 8. The residue

Six things that are each one paragraph and none of which changes a design.

**A body read twice answers the caller 400, and the tree says whose fault it is.** `take_body`'s own
comment reads *"The client's request was fine; the handler asked for the body twice"*
(`crates/ulo/src/extract/from_context.rs:97-99`), and it logs at `error` for that reason. Probed with
a guard that reads the body and a handler that then takes `Json<T>`: `400 {"error":"Extraction
failed","details":"…found the request body already read by something earlier in this handler"}`.
`body_already_read.rs:88-91` pins the same answer — a client-error status for a fault the
source has identified as the application's. gRPC states the opposite rule for the identical
condition and follows it: `RequestError`'s doc says *"Each is a fault in the handler or the dispatch
rather than in the call, so the generated method answers `Internal`"*
(`crates/ulo/src/grpc/runtime.rs:460-462`). One condition, two rules, and the rule HTTP follows is
not written anywhere. F164.

**`Validated<E>` flattens what it wraps to a string.** `ValidationError::ExtractionError(String)`
holds `e.to_string()` of the inner extractor's error (`crates/ulo/src/extract/validated.rs:149-151`),
so `Validated<Json<T>>` cannot report that the body had already been read as anything but text, and
the validation half puts `validator::ValidationErrors` — which is structured, and which
`Error::details()` exists to carry — into a `Display` string as well. F175.

**`Path` discards the error it was given and reports the map instead.** The struct branch is
`if let Ok(v) = serde_urlencoded::from_str::<T>(&encoded)` (`crates/ulo/src/http/extract/path.rs:72`),
so a `Path<Mixed>` whose `id` field will not parse answers *"Failed to deserialize path params from
{...}"* naming the whole parameter map, where `Query<T>` on the same failure forwards serde's message
naming the field. `PathError::NotFound` is constructed nowhere — its only occurrence is its own
`Display` arm — so the variant that exists to say a parameter was missing is unreachable and
`ParseError` says it instead. F174.

**Every route wrapper clones the request parts whether or not anything reads them.**
`generate_route_wrapper` emits `let _req_parts = __ctx.request().clone();` unconditionally
(`crates/ulo-macros/src/controller_macro/instance_injection.rs:392`), and the only reader is the
`#[param]` marker's codegen (`extracts_marker_params.rs:190`). `RequestPart` is
`http::request::Parts`, so the clone copies the header map, the URI and the extension bag on every
request to every route. The leading underscore is what keeps it from warning. Cost unmeasured, and
the performance pass owns whether it matters; the fact that it is emitted for a branch most handlers
do not take is this pass's. F177.

**`ExtractorKind::Optional` generates what the blanket impl already does.** Its arm emits
`<Inner as FromContext<_>>::extract(ctx).await.ok()`
(`crates/ulo-macros/src/controller_macro/extractor_params.rs:286-290`); the `_` arm on the written
type `Option<Inner>` reaches `impl<C, T: FromContext<C>> FromContext<C> for Option<T>`
(`extract/from_context.rs:51-59`), whose body is `Ok(T::extract(ctx).await.ok())`. The two are the same expression and carry the same `CONSUMES`. Both spellings are in §4's accepted
list — `Option<Json<Dto>>` written out takes the `Optional` arm, the same type behind a `type` alias
takes the `_` arm, and both compile. That the values agree is Read off the two expressions rather
than run. Folded into F166.

**ADR-0023 tells a reader the compile-time check is impossible, and does not point at the record that
built it.** Its *Roads not taken* argues that a macro cannot classify by trait and that "the
diagnostic it exists to produce names the two colliding parameters and their spans — neither
survives being raised from inside a macro's own output". ADR-0040 then produced exactly that
diagnostic, from inside a macro's own output, by reading an associated const. 0023 carries no
pointer to 0040 — the cross-references into 0023 are from 0015, 0038, 0041 and the README — so the
extraction ADR, which is the one a reader opens first, states as settled a thing a later record
overturned. The convention is one line under the earlier record's title
(`project-adr-refinement-convention`). F178.

## 9. Decisions — settled 2026-09-22

Six, all answered. F84 is first because §2, §3 and half of §8 are downstream of it. Each records what
was chosen and what was rejected, so the build reads a decision rather than a recommendation; the
rejected alternatives are kept because they are the part nothing else in the tree will carry.

**D1–D5 are this build's. D6 parks a feature and D7 is not this build's at all** — it is a DI
decision this pass raised, recorded as D14 in `DI_AUDIT.md` and built in that handoff's group F.

### D1 — an extraction failure reaches the error chain, carrying its type. SETTLED.

The question the pipeline audit handed over. Its own §2 was the same shape and was decided in favour
of the chain; this goes the same way, on all four transports. The bound
`FromContext::Error: fmt::Display` becomes `FromContext::Error: ulo::Error`. Each wrapper then stops
calling `.to_string()` and lifts the error the way a handler's own error is lifted:
`ExecutionResult::Err(HttpError::from(e))` reaches the existing blanket
`impl<E: ulo::Error> From<E> for HttpError`, which wraps it as `AppError`, and every dispatcher
already unwraps `AppError` before offering it to the chain — so `#[catch(JsonError)]` matches with
nothing written for it. An unclaimed failure renders through `render_error`, which reads
`Error::kind()`.

*What it closes:* F84; F1 on all three transports; C11; §2's statuses, because a kind is finally
there to read; F164, because `BodyAlreadyRead` can name `Internal` where `JsonError` names
`BadRequest`; and F175's flattening, because a `Validated` failure can put `ValidationErrors` in
`Error::details()` rather than in a `Display` string.

*What it costs:* eleven `impl ulo::Error` blocks in core, one per extractor error, each of which is
a `kind()` and nothing else. `BodyExtractionError<E>` gains `std::error::Error`, a `source()` and a
forwarding `kind()`. It is breaking for every custom extractor in every application, which is D13's
case rather than an objection.

*Rejected — keeping the bypass and recording why.* The argument available for it is that extraction runs
before the handler, so there is no application error to shape. The framework refutes it three times:
a guard's refusal is not the application's error and reaches the chain as `GuardRejection`; an
unrouted RPC pattern reaches it; a panic in any segment reaches it. And an application does want to
shape this one — a service answering `application/problem+json` wants its 400 in that shape too.

*Rejected — reaching the chain with the error erased.* `HttpError::bad_request(e.to_string())` needs
no trait-bound change and makes `#[catch(HttpError)]` work. It fixes the seam and nothing else: the
status stays a literal, §2 stays unreachable, F164 stays, and the three non-HTTP transports stay at
`Internal`. It is the cheap half of what was chosen and buys the smaller half of what that buys.

### D2 — the markers stay as syntax and stop being a code path. SETTLED.

`get_extractor_params` already lowers `#[body] x: T` to `Body<T>` for the one-body assertion
(`extractor_params.rs:104-113`); lower the other two the same way and emit every parameter's binding
from one function, in signature order. A marker and a typed extractor then share a signature, the
`use_extractors` switch goes, and the marker path's six error strings and eight hardcoded 400s go
with it.

*What it closes:* A16, A17, C3, and the three-diagnostics-for-one-mistake behaviour in §4.

*Not proposed — deleting the markers.* The user recorded them as a deliberate alternative syntax
rather than legacy.

*Rejected — keeping two paths and documenting the constraint.* `markers.mdx` documents the mix as
supported, so the documentation change is to withdraw a feature rather than to describe one, and the
three diagnostics stay.

### D3 — `#[param("id", default = "…")]` is refused at expansion. SETTLED.

A path parameter is present whenever the route matched, so
the default can apply only when the marker names a parameter the route does not declare — a typo. A2
offers both options; this is the argument for the second. `#[query]`'s default is the opposite case
and stays, with its literal parsed at expansion rather than per request (A3) and with a missing value
distinguished from an unparseable one (C4).

### D4 — one `Content-Type` policy for both body extractors, answering 415. SETTLED.

Both read the header, both accept `application/json` and every `+json` suffix, both treat an absent
header as unknown-and-try — which `Body<T>` already does with one — and both answer 415 otherwise. It
fills §2's one empty cell, and RFC 6838 §4.2.8's suffix convention is what makes `application/problem+json`
readable by the same extractor — which matters because ADR-0053 gives ulo's own error body that type.

*Rejected — leaving `Json<T>` permissive and giving only `Body<T>` the 415.* It keeps two body
extractors with opposite policies, which is F176 agreed to rather than removed.

### D5 — the WebSocket connection hooks read their parameters through `FromContext`. SETTLED.

`#[on_connect]`, `#[on_disconnect]` and `#[after_init]` read their parameters through
`FromContext<WsContext>` like every other handler in the framework. It removes the last
positional surface, closes A6 with nothing written for it, and makes `Session` reachable from the
hook that runs immediately after the guards that populate it.

*Rejected — a positional slot for `DisconnectReason` alone.* It closes A6 and leaves the hooks unable
to take `Session`, `Extensions` or a custom extractor, and leaves `&WsClient` in a connect hook
meaning what `WsClient` means in a message handler on the same gateway.

### D6 — a single typed header. PARKED, with a trigger and an implementation. SETTLED 2026-09-22.

Not built now, and the reason is a dependency rather than ergonomics.

**What the comparators do**, fetched this session rather than recalled: Nest has `@Headers(name?)`,
both the whole map and one named header; axum has `HeaderMap` as an extractor and `TypedHeader<T>` in
`axum-extra`; actix-web has `web::Header<T>`; poem has `web::Header<T>`. All four ship one and three
ship a *typed* one. ulo already has the first half — `Request` in parameter position is Nest's
`@Headers()` (§6) — and has no spelling for the second.

**Why not now.** `Header<String>` buys nothing over `req.header("x")`. The version worth building is
the one where the *type* does the parsing — `TypedHeader<ContentType>`, `Header<IfNoneMatch>` — and
the grammars for those are RFC 9110's, which means the parsing is a solved, specified problem that
ulo would be re-doing. The typed layer is not in the `http` crate: it is `headers`, a separate
hyperium crate depending on `http` ^1.0 and shipping around forty header types. axum-extra wraps it;
actix and poem each wrote their own trait instead.

**The trigger to unpark**, stated so it is recognisable: the first header whose *value* needs
parsing rather than reading — content negotiation over `Accept`, `Range`, or a conditional request
over `If-None-Match` / `ETag`.

**The implementation, decided with the park:** `headers`, behind a feature, not a ulo trait.
Re-deriving forty grammars offers nothing the crate does not already have, which is the same test
that keeps `serde_urlencoded` and `multer` in the tree.

Nothing is blocked meanwhile: `Request` reads a header at the signature today, and `RequestPart`
will after DI's PR 22.

### D7 — an injectable is shared, not copied. SETTLED 2026-09-22, and recorded as DI's D14.

Raised here because §6 could not settle `Request` without it, and it is not this build's: it is a
statement about what a provider hands out. `Arc<T>` for singleton and execution scope, by value for
transient, with the field written `Arc<T>` at the injection site.

**The argument in one line:** the scopes are read as promises about identity and they are promises
about construction count. A singleton is built once and copied to every injection point
(`instance_injection.rs:363`, `:372`); `ExecutionCache` stores an `Arc` and `get` ends in `.cloned()`
(`execution_cache.rs:39-52`). Both layers already hold the `Arc` and discard it, so this keeps an
indirection rather than adding one.

*Rejected: the macro rewriting the field*, storing `Arc<T>` behind a field written `T`. It keeps
every signature compiling and makes the sharing invisible, which is the property the decision exists
to make visible. *Rejected: documenting the model*, which is the state today and reads as a
workaround notice — `Arc<Mutex<_>>` as advice about how to defeat the copy.

What it carries: the provider macro's added `#[derive(Clone)]` goes with it, which is F7's cause;
`Request` loses its reason to exist (F180); `Extension<T>` must not be wrapped again, because the
`Extensions` it holds is already `Arc`-backed, and its per-type registration goes (F181). The full
record, with the three-assertion test that makes it checkable, is `DI_AUDIT.md` D14.

## 10. Out of scope, and why

**The query parser's dependency and percent-decoding** — F117 and F111. The handoff assigns both to
ADR-0052's build, which moves matching and decoding into core. F117's `serde_urlencoded` swap reaches
`Path` as well, because `Path`'s struct branch round-trips through the same crate
(`extract/path.rs:70-72`), so whichever crate that build picks decides both extractors. Nothing here
changes that.

**The failure body's shape** — ADR-0053. This pass takes the status and leaves the envelope. The two
meet in one place: D1 makes the extractor's error reach `render_error`, which is the function 0053
rewrites, so 0053's envelope is what an extraction failure will render as once D1 is built. Neither
record needs to name the other.

**A wrong `Payload<T>` on gRPC** — F75, and C16 beside it. ADR-0043 records the compile-time check as
a deliberate follow-up and `project-grpc-parked-followups` records the user parking it on 2026-09-16.
It is the same class as §4's spelling questions and it is already answered.

**The request-body ceiling** — F112 and F116. The HTTP audit owns both; §1's capability table is where
the policy goes, and the multipart constraints derive from it.

**Request-side streaming on RPC** — `project-rpc-request-side-streaming`. `Inbound<T>` is gRPC's
extractor for a caller's stream and RPC has no analogue, which is a transport capability rather than
an extraction question.

**What a provider hands out, and what `Request` is for.** Taken here and settled, not deferred — the
earlier draft of this section sent it to the DI pass, which had already closed with D1–D13 and would
never have seen it. `Request` and the extension bag are both DI surfaces that extraction reaches, so
this pass raised them and the DI record carries the decisions: **D14** in `DI_AUDIT.md` makes an
injectable shared rather than copied, which is what lets `RequestPart` replace `Request` (F180) and
what says `Extension<T>` must not be `Arc`-wrapped (F181). `DI_BUILD_HANDOFF.md`'s group F is the
work. The extension **bag** stays on its own merit: it is the only extractor with
`impl<C: ExecutionContext> FromContext<C>` — one impl, every context, no transport in its type.
