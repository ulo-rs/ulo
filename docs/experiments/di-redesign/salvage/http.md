# Salvage: HTTP

**Subject.** The September Smile HTTP audit: the adapter seam (who matches a route), a five-adapter
wire probe, the response header and status model, the request-body ceiling, problem details, and
the fourteen-PR build that was to follow. Written 2026-09-20..24 against master `654c7183`, where
HTTP was served through five adapter crates (axum, actix, salvo, poem, rocket) that each owned
routing.

**Sources read in full.**

| File | Words |
| --- | --- |
| `source/http/HTTP_AUDIT.md` | 6,080 |
| `source/http/HTTP_BUILD_HANDOFF.md` | 789 |
| `source/http/artifacts/spike-dispatch-point-upgrade.patch` | 959 |

Also read for this subject: `FRAMEWORK_GAPS.md` entries F4, F22, F27, F65, F69, F81, F87, F103–F120,
F125, F243, F244; `source/BUILD_SEQUENCE.md` rows 76–106 and 131 and rulings S1, S4, S6, S8, S10,
S12; ADRs 0052, 0053 and 0054 at commit `9d3fb9c2` (read with `git show`).

**Checked against.** Branch `experiment/di-redesign` at `331c35b9`, 2026-11-01: `crates/ulo-http`,
`crates/ulo-http-hyper`, the five embedding crates `crates/ulo-http-{axum,actix,salvo,poem,rocket}`,
`crates/ulo-http-conformance`, `crates/ulo/src/execution`, and `docs/experiments/di-redesign/transports/DESIGN.md`
§3 (cited below as "T§"). Statuses come from reading; nothing was built or run.

**One correction to the brief.** The five adapter crates exist at HEAD under their old names, but
as rebuilt *embeddings* (T§3.8): the host owns the socket and calls into a router `ulo-http` owns.
None of the old code survives in them. Where this file says "the old adapter" it means the crate at
`654c7183`.

---

## Summary

| Status | Count |
| --- | --- |
| built | 46 |
| superseded | 26 |
| open | 15 |
| dropped | 5 |
| UNVERIFIED | 5 |
| **total** | **97** |

The rebuild took the audit's central decision (D1: the framework owns the route table) and built
it in a different shape: one first-party backend (`ulo-http-hyper`) plus five embeddings, rather
than five adapters behind a `dispatch` SPI. Most of the wire defects the audit measured close by
construction at HEAD.

**The five most valuable open items.**

1. **D2 reversed without its argument (§F, D2).** The audit decided `%2F` inside a path parameter
   stays encoded, because a decoded separator turns a one-segment capture into a traversal
   primitive for the common sink, a path join. HEAD decodes `%2F` to `/` inside the segment
   (`crates/ulo-http/src/router/pattern.rs:8-10`), and the recorded reason
   (`divergences/race2a-H.md` §1, `transports/DIVERGENCES.md` T22) concerns invalid UTF-8 only. The
   traversal argument was never weighed.
2. **A running HTTP handler is not told its caller left (§I, D3-ii-HTTP).** Nothing guards the
   pipeline future before the response head exists: the service's only `Drop` impls are `ExecBody`
   and `Buffered` (`crates/ulo-http/src/service.rs:748`, `:948-955`), `Execution`'s `Drop` sets
   `ended` and releases the cache without cancelling (`crates/ulo/src/execution/mod.rs:412-417`),
   and the service's doc states "A peer leaving before the head is written is a connection failure,
   not an incomplete body" (`service.rs:75-76`). Work holding a cancellation clone runs on. Not in
   the ledger that I found.
3. **The §2 wire table is not in the conformance suite (§B, W14).** The suite's 23 scenarios
   (`crates/ulo-http-conformance/src/lib.rs:349-372`) carry none of: two `Set-Cookie` lines, a 204
   carrying a body, status `599`, an invalid header value, `//probe`, an unknown method, a request
   without `Host`, `Content-Length` with `Transfer-Encoding`, and the agreeing half (absolute-form,
   `Expect: 100-continue`, HTTP/1.0, dot segments, duplicate `Content-Length`). These are the cases
   on which five independent servers were measured to disagree; the five embeddings now exist to be
   held to them.
4. **ADRs 0052, 0053 and 0054 exist only on an unreachable commit (§J, HO2).** `9d3fb9c2` is on no
   branch (`git branch -a --contains` answers nothing; `git fsck --unreachable --no-reflogs` lists
   it). A `git gc` deletes the only copy of three records whose Roads-not-taken sections hold
   arguments the audit memo states in shorter form. They are not in `source/`.
5. **A 204 carrying a body may still reach actix's wire (§B, W10).** The service discards nothing
   for a status that forbids content: it marks the body "unwritten" and passes it to the host
   (`service.rs:234-243`, `:545-553`). At `654c7183` actix-http wrote such a body after the head,
   probed as `…\r\n\r\nLEAKHTTP/1.1 200 OK…`, a response-splitting primitive. Whether actix-http
   3.18.12 still does is a run.

---

## A. The seam (`HTTP_AUDIT.md` §1)

| ID | What | Status | Evidence at HEAD | Value to extract |
| --- | --- | --- | --- | --- |
| H1 | The correlation: the transports that kept the matching decision are the ones that can report a miss; the rule *an adapter routes down to the smallest unit the framework can name, and no further* | built | T§3.2 ("`fw-http` owns the router, so every backend routes identically"); `crates/ulo-http/src/router/mod.rs:1-8` | The rule as a test for any future transport (below) |
| H2 | The three answers to ADR-0007's rejection of a ulo-owned router | superseded | The rebuild owns the router without an ADR answering 0007; the old ADR set is not carried | The argument (below) |
| H3 | Proposed SPI: `HttpAdapter { capabilities, into_lifecycle }`, `ServeContext::dispatch(HttpRequest) -> Dispatched { Respond, Upgrade }` | superseded | `Backend` + `AppService::call(Request) -> Response` (T§3.7, `crates/ulo-http/src/backend.rs`, `service.rs`); upgrades through `Request.upgrade: Option<OnUpgrade>` and `UpgradeHandler`, read before route matching (T§3.5) | The negotiation argument (below) |
| H4 | `AdapterCapabilities { streams_responses, same_port_upgrade, adopts_listener, max_request_body }`, checked once before a socket opens | superseded | `BackendLimits { upgrades, h2c, inherited_sockets, port_zero, tls }` (`backend.rs:62-76`) and `EmbedLimits` (`embed.rs:123-152`), enforced in `prepare` | `streams_responses` has no successor field because every host must stream (`sse_delivered_as_produced` fails a buffering host by its 10 s read deadline, `CORE_TEST_PLAN.md:367`); `max_request_body` has none because the limit is the app's (D5) |
| H5 | `matchit` 0.8 as core's matcher; unverified whether it accepts all five adapters' inputs | superseded | Own matcher, `router/pattern.rs`; `matchit` appears in `Cargo.lock` only under axum | The unverified question is moot |
| H6 | What the SPI change closes and costs; "the break is free before first publish" | dropped | Process argument for a build that did not run | — |
| H7 | The spike: same-port upgrade decided from the framework's request at the dispatch point, on poem and salvo | built | Hosts hand `OnUpgrade` to the app: salvo `handler.rs:39-43`, poem `endpoint.rs:32`, axum `lib.rs:60`; `upgrade_echoes_a_frame` stamped into every host's suite | Why it worked (below) |
| H8 | Rocket's magic response header `x-ulo-rocket-ws-route` as `Dispatched::Upgrade` hand-rolled | superseded | Rocket upgrades through its `IoHandler` (`crates/ulo-http-rocket/src/lib.rs:28`, `convert.rs:50-55`); `grep -rn x-ulo-rocket crates` finds nothing | A return type that cannot express a case gets an out-of-band channel; the rebuild's `Request.upgrade` slot is the in-band one |
| H9 | What stays the adapter's: accept, parse, framing, `Expect: 100-continue`, TLS, HTTP/2, the handshake, writing | built | T§3.7 (`Backend`) and T§3.8 ("Routing, extraction, pre-dispatch, dispatch and error rendering run inside the app either way") | Rocket's parser-level 400 stays below the line: W5 |
| H10 | "Four of the five are the same protocol stack (hyper); two implementations wrapped five ways" | superseded | T§3.8 "Engines and adapters": three engines, hyper 1.x, actix-http, and **hyper 0.14 under rocket 0.5**; `Cargo.lock` carries `hyper` 0.14.32 and 1.11.1 | Correction: rocket is a third engine, not a hyper 1 sibling |
| H11 | Interop, not implementation, as the reason to keep five | built | T§3.8: five embeddings (tower for axum, native handler traits for the other four), `Miss::Forward` returning a miss to the host | — |
| H12 | `ulo-http-hyper` as the reference and default; hyper supplies protocol but not serving policy | built | `crates/ulo-http-hyper`; T§3.7 "the one backend, the reference and the default"; policy now explicit: `header_timeout`, `handshake_timeout` (30 s), `max_inflight` (1,024), `ACCEPT_BACKOFF` (1 s) (T§3.6, T§3.7) | — |
| H13 | The five as a conformance asset: one implementation hides every defect | built | `ulo-http-conformance`, run by eight suites (hyper on tokio and smol, axum, actix HTTP/1.1 and h2c, poem, salvo, rocket), `CORE_TEST_PLAN.md:367` | The suite lacks the cases that produced the evidence: W14 |
| H14 | "One extra match per request on axum and poem, unmeasured" | UNVERIFIED | Every host now routes through `Router::route` (`router/mod.rs:194-215`); no measurement found | Needs a benchmark; belongs with the performance subject |

**H1, value.** The test, stated so the next transport is decided rather than argued: the
transport that makes the routing decision is the only one that can report a miss, because
reporting it requires having made it (`HTTP_AUDIT.md` §1, the four-transport table: RPC and
WebSocket raised `Unrouted`; HTTP and gRPC, which delegated matching, could not). At HEAD HTTP
reports the miss as `CallError(NotFound)` with source `fw_http::NoRoute` (T§3.2), which is the rule
applied.

**H2, value.** ADR-0007 (July 2026) rejected a ulo-owned router as making "the five native routers
dumb byte-pumps" that "re-implement path matching the hosts already do well. Nest never does this."
The audit's answers, in increasing weight: (1) the same ADR named `matchit` legitimate "for a future
adapter-independent portless handler" and called socket binding "the natural seam for portless
dispatch"; (2) "Nest never does this" is the argument class the project already ruled against (F65:
a popular framework is not a standard; Nest delegates to two Node servers with compatible
semantics, ulo delegated to five Rust servers with five); (3) "the hosts do it well" was falsified
by the §2 probe, which did not exist in July: the hosts match well individually and do not agree,
and salvo and actix kept a second route table for 405 while rocket had no usable native routing.
The precedent ran the same way: ADR-0011 (trailing slash, "five routers have five strictness
models"), ADR-0012 (`{param}`, "ten translation points"), ADR-0007 itself (the middleware anchor)
each moved a piece of routing into core because the adapters could not agree.

**H3, value.** The user's ask framed a four-message negotiation (*I have a request / I cannot take
that / give me the pipeline / here is the response*). The audit's reduction: the only thing an
adapter can refuse is a capability, a capability does not change between requests, so the refusal
is static and belongs in a declaration read once; what remains per request is the framework
answering *what to do with this*, one call and one enum. The case the negotiation was invented for,
a same-port upgrade, is the enum's second variant. HEAD has the same split (static `BackendLimits`
/ `EmbedLimits` in `prepare`; one `AppService::call` per request) with the upgrade carried in the
request rather than the answer.

**H7, value (the spike's mechanism, `HTTP_AUDIT.md` §1 and the patch).** The patch removes the
native WebSocket mount on poem (`route.at(..., UloWsEndpoint)`) and salvo
(`Router::with_path(..).goal(UloWsHandler)`) and decides `Upgrade` in a function that sees only the
framework's `HttpRequest` (method `GET` and an exact path match against the WS route list), then
performs the handshake from the native request shell retained across the global chain
(`WebSocket::from_request_without_body(&req)` on poem; `WebSocketUpgrade::new().upgrade(&mut sreq,
&mut sres, ..)` on salvo). Falsified before believed: `if false && …` on the decision made
`ws_same_port_upgrade_and_echo` fail with `404 Cannot GET /ws` on both. Why it worked: the upgrade
is driven entirely by state in the request (poem's shell "keeps state http parts cannot carry — the
WebSocket upgrade slot"; salvo's `mem::take` moves it), and the 101 is ordinary data that
round-trips through a status/headers/body response. The claim holds at HEAD in a different shape:
the host takes `hyper::upgrade::OnUpgrade` out of the request's extensions and hands it to the app
as `Request.upgrade` (salvo `handler.rs:39-43`), the app's `UpgradeHandler` decides from the head,
and the 101 goes back as an ordinary `Response`.

---

## B. One program, five wires (`HTTP_AUDIT.md` §2; F106–F114)

Probe at `654c7183`: one module registering `GET /probe`, `GET`+`POST /both`, `GET /seg/{id}`, raw
TCP so the bytes were the evidence, every probe falsified by disabling it.

| ID | Row (old result: axum / actix / salvo / poem / rocket) | Ledger | Status | Evidence at HEAD |
| --- | --- | --- | --- | --- |
| W1 | `GET //probe`: 404 / 405 / **200** / 404 / **200** | F106 | built | `split("//probe")` yields an empty first piece (`router/pattern.rs:221-235`), the piece count differs from the pattern's, no match, 404. Not pinned by a conformance case |
| W2 | `HEAD /probe`: 200 / **405** / **405** / 200 / **405** | F107 | built | `router/mod.rs:203-207` (GET answers HEAD), `service.rs:559-567` strips the body and keeps an exact length; pinned `head_answered_from_get` |
| W3 | `OPTIONS /probe`: 405 on all five | F107 | built | `router/mod.rs:209-210`, 204 with `Allow`; pinned `options_answered_204` |
| W4 | `POST /probe`: body empty / envelope / envelope / text / envelope; `Allow` `GET,HEAD` / `GET` / `GET` / **absent** / `GET` | F106 | built | `router/mod.rs:212`, `allow_of` `:282-293` (declaration order, `HEAD` after `GET`, then `OPTIONS`); 405 problem document (`render.rs:150-157`); pinned `method_not_allowed_with_allow` |
| W5 | `PROPFIND /probe`: 405 ×4, **rocket 400 HTML** | F106 (501 clause) | open | HEAD answers 405 for any unrecognised method on a matched path (`router/mod.rs:212`); RFC 9110 §9.1: an origin server receiving an unrecognised or unimplemented method "SHOULD respond with the 501 (Not Implemented) status code". Rocket 0.5 still mounts nine methods only (`crates/ulo-http-rocket/src/handler.rs:27-36`), so its parser answers an unknown method before the app (Read, not run). F106's text describes the old code |
| W6 | `get /probe` (lowercase): salvo and actix report it as `GET` | F106 | built | `http::Method` is case-sensitive and the router compares exactly; actix's bridge re-parses the method bytes as sent (`ulo-http-actix/src/service.rs:93`). Not pinned |
| W7 | `/seg/a%2Fb`: `a/b` / `a%2Fb` / `a/b` / `a/b` / `a%2Fb` | F111 | superseded | Decoded to `a/b` inside the segment (`router/pattern.rs:8-10`, T§3.2); pinned by `path_parameter_percent_decoded` (`a%20b%2Fc%2520` is `a b/c%20`). Contradicts D2: §F |
| W8 | `/seg/a%20b`: `a b` ×4, **rocket `a%20b`** | F111 | built | Same decoding on every host; pinned |
| W9 | Two `Set-Cookie`: last only ×4, **poem both** | F108 | built | `Response = http::Response<HttpBody>` (`response.rs:15`), a `HeaderMap`; `WithHeaders` appends (`response.rs:124-131`); hosts append: rocket `raw_header_adjoin` (`convert.rs:49`), salvo `HeaderMap::extend` (`handler.rs:78`), actix `append` (`service.rs:129-134`), axum and the hyper backend pass `http` 1 types through. Not pinned |
| W10 | 204 with a body: `Content-Length: 18` / **body on the wire** / clean / clean / `Content-Length: 18` | F110 | UNVERIFIED | Core now names the rule (`forbids_body`: 1xx, 204, 304, RFC 9110 §6.4.1, `service.rs:535-538`) and logs a streaming body under such a status at `warn` (`:545-553`), but passes the body to the host rather than replacing it with an empty one (`:234-243`; `ExecBody::new` only marks it ended, `:842-844`). Hyper 1 and actix-http 3.18.12 decide what reaches the wire. Needs the old pipeline probe re-run per host |
| W11 | Status `599`: `599` ×4, **rocket 500** | F114 | built | `http::StatusCode` (100–999); rocket `Status::new(parts.status.as_u16())` (`convert.rs:44`); actix `from_u16` (`service.rs:126-127`). Not pinned |
| W12 | Header value with CRLF: dropped / **500** / dropped / dropped / **0 bytes, connection closed** | F113 | built | `HeaderValue` refuses CR/LF at construction; `WithHeaders` fails the reply as `IntoReplyError` before anything is written (`response.rs:124-131`); `Created` likewise (`:107-113`) |
| W13 | 8 MiB body: 200 / **413 (actix text)** / 200 / 200 / 200 | F112 | built | `body_limit` 2 MiB unset, per-route `#[meta(BodyLimit(..))]` (`limits.rs:13-18`, T§3.6); `BodyStream` ends at the limit with `ExtractError::TooLarge` (`extract/body.rs:134-160`); rocket buffers under the embedding's `body_limit` (`ulo-http-rocket/src/lib.rs:20-23`); pinned `chunked_body_over_the_limit` |
| W14 | The agreeing half: the 404 envelope's shape, duplicate `Content-Length` → 400, absolute-form request lines, `Expect: 100-continue`, chunked bodies, HTTP/1.0, `/./probe` and `/x/../probe` → 404, no injected header line on the wire | — | open | None of these, nor W1, W6, W9, W10, W11, W12, I1 or I2, is a scenario in `ulo-http-conformance` (`src/lib.rs:349-372`) |
| W15 | (new, found while checking W9) Rocket converts each header value through `String::from_utf8_lossy` (`ulo-http-rocket/src/convert.rs:49`) | — | open | A valid `HeaderValue` carrying obs-text bytes (0x80–0xFF, which RFC 9110 §5.5 admits) reaches the wire with U+FFFD substituted on rocket alone. Not filed |

**W14, value (test ideas, each stated so it can be written).** Every row below was measured on five
servers at `654c7183` and is a scenario the eight-suite harness can stamp per host:

- `GET //probe` against `GET /probe`: expect 404 (W1). A host that canonicalises `//` before the
  app sees it would route `//admin` to the admin route past an unscoped entry matching the path as
  sent; that is the defect class ADR-0011 closed for trailing slashes.
- `PROPFIND` and lowercase `get` against `GET /probe`: expect the framework's chosen answer (405 or
  501, W5) and a `detail` naming the method as sent.
- A handler pushing two `Set-Cookie` values: expect two field lines. The same scenario with
  `Vary: Accept-Encoding` set by the handler and `Cors` registered: expect `Vary` to list both
  (the old CORS breakage, F108).
- A handler answering 204 with a known-length body and one with a streaming body, then a pipelined
  `GET` on the same connection: expect the second response to start cleanly after the first head
  (RFC 9112 §6.3 item 1). The old actix leak read `…\r\n\r\nLEAKHTTP/1.1 200 OK…`.
- Status `599` and `299`: expect them on the wire unchanged.
- A header value containing `\r\n`: expect the reply refused where it is built, the client reading
  the framework's 500 problem document, never a dropped header or a closed connection.
- Requests the five servers agreed on, kept as a floor: absolute-form `GET http://host/probe`,
  `Expect: 100-continue`, chunked request bodies, HTTP/1.0 requests, `/./probe` and `/x/../probe`
  (404), duplicate `Content-Length` (400).
- I1 and I2 below.

---

## C. What is inherited, not ulo's (`HTTP_AUDIT.md` §3)

| ID | What | Status | Evidence at HEAD | Value to extract |
| --- | --- | --- | --- | --- |
| I1 | An HTTP/1.1 request with no `Host` was answered 200 by all five. RFC 9112 §3.2: "A server MUST respond with a 400 (Bad Request) status code to any HTTP/1.1 request message that lacks a Host header field." Enforceable in a global entry in four lines; whether to was left as a decision | UNVERIFIED | No `Host` check in `crates/ulo-http/src` or `crates/ulo-server-hyper/src` (grep). Hyper 1.11.1's and actix-http 3.18.12's behaviour needs a run | The spec quote; the decision is still unmade (designer question 4) |
| I2 | `Content-Length` and `Transfer-Encoding` on one request: accepted by four (TE wins, 200), refused by actix. RFC 9112 §6.3: "the Transfer-Encoding overrides the Content-Length. Such a message might indicate an attempt to perform request smuggling … and ought to be handled as an error." | UNVERIFIED | No ulo check found. With a core-owned service the stricter reading is one check in `AppService::call` (`service.rs`) | The spec quote; the observation that the stricter reading costs one check once the framework owns the entry point |

---

## D. Findings that own no wire row

| ID | What | Status | Evidence at HEAD | Value to extract |
| --- | --- | --- | --- | --- |
| F4 | A duplicate route panicked out of `bind()` on axum; the other four served one of the two in hash-map order | built | `prepare` refuses two routes with the same method and normalised pattern, and two patterns differing only in a parameter's name at one position, naming both handlers (`router/mod.rs:1-3`, T§3.2) | The second refusal (`/u/{id}` beside `/u/{name}`) is the rebuild's own addition |
| F22 | No app-wide seam to restyle the envelope without enumerating error types | superseded | Error handlers return a `CallError` and the transport renders it by kind (T§2.4); `render.rs::render_as_is` | — |
| F27 | actix capped request bodies at 256 KiB (actix-web `PayloadConfig`), unconfigurable | superseded | The actix embedding pumps the raw payload through a bounded channel (`ulo-http-actix/src/lib.rs:24-25`, `service.rs:80`); the app's `body_limit` decides | — |
| F65 | HTTP error body was NestJS's `HttpException` shape | built | RFC 9457 `application/problem+json` (`render.rs:165-190`) | — |
| F69 | 422's title was RFC 4918's phrase; `Timeout` → 504 is a gateway status for a handler's own budget | built | `title` answers "Unprocessable Content" and "Content Too Large" (`render.rs:199-207`); 504 kept with its rationale written beside the table: "the server ran out of time, usually waiting downstream; 408 means it gave up waiting for the client's request" (`render.rs:20-21`) | The 504 question is now decided in the source; the ledger entry's second half can close against `render.rs:20-21` |
| F81 / F125 | A global middleware's failure or panic reached no error handler and answered a literal 500, losing the status it named | built | Both pre-dispatch sub-steps run inside the error chain; an `Err` arrives as a `BoxError`, a panic as `PanicRecovered`; on a miss only the global handlers apply (T§3.3) | — |
| F87 | A `#[catch(MyError)]` handler could not produce the canonical envelope (`render_error` private) | superseded | The renderer stays `pub(crate)` (`render.rs:47`), and is not needed: a handler returns a reshaped `CallError` and the transport renders it (T§2.4, T§3.6 "An error reshaped to another kind renders by that kind") | — |
| F103 | `streams_responses` guarded nothing; keep or delete undecided | superseded | No such hook; every host must stream (`sse_delivered_as_produced`) | — |
| F104 | A collecting adapter could not refuse a streaming body chosen at runtime | dropped | No collecting host exists: rocket buffers request bodies but streams responses (`ulo-http-rocket/src/convert.rs:32-34`) | — |
| F105 | The framework never learned of a miss and could not tell its own 404 from a handler's | built | `NoRoute` source on the miss's `CallError` (`crates/ulo-http/src/miss.rs`, T§3.2 "`source_as::<NoRoute>()` tells a miss from a handler's own `NotFound`"); pinned `not_found_with_no_route` | — |
| F106 | Three adapters reimplemented matching; the envelope was hand-written five more times | built (except W5) | One router, one renderer | — |
| F109 | `Cors` added no `Vary: Origin` to a request without `Origin` | built | `cors.rs:170-175` adds it on every response, preflights `Vary` on `Origin`, `Access-Control-Request-Method`, `Access-Control-Request-Headers` (`:134-137`) | RFC 9111 §4.1 (reuse is bound to the stored response's `Vary`) is the mechanism the comment at `cors.rs:172-173` states |
| F115 | No cookie surface | open | `grep -rni cookie crates/ulo-http/src` finds nothing. Ledger F115 holds it (its text cites old paths) | RFC 6265 defines both directions. The audit's ordering constraint is satisfied: a multimap now exists (W9), so a `Cookies` extractor and a typed `SetCookie` reply are buildable |
| F116 | The multipart reader had no constraints | open (partial) | Total size bounded: `Multipart` reads through the route-limited `BodyStream` (`extract/body.rs:210-212`). No per-field ceiling, no field-count limit, no `multer::Constraints` passed | One part may still be the whole body; F116's per-field override on the extractor is the remaining half |
| F117 | A repeated query key could not be extracted | open | Own form parser (`extract/de.rs`). A repeated key into a struct field is refused as "`tag` is given more than once" (`de.rs:100-102`); a `Vec<String>` field reads one value as a string and fails, since `Value` forwards `seq` to `visit_str` (`de.rs:270`). Ledger F117 holds it (old text) | The failure is now a named refusal rather than last-wins; sequence support is still absent. S1's crate choice (§I) is the recorded alternative |
| F118 | A middleware is pre-routing or path-scoped, not both; `/admin` auth must answer for misses under `/admin` | open | Scoped entries run only after a route matched (T§3.3: "Then the route is matched … Then the entries whose pattern matches the route run"); a miss under `/admin/*` runs only unscoped entries and answers 404 | The disclosure: an anonymous caller reading 404 against 401 learns which admin paths exist. Designer question 3 |
| F119 | `for_route` took a registered pattern; a concrete path matched nothing, silently | built | `prepare` refuses a scope covering no route the entry does not exclude (T§3.3, `ulo_http::stage::Stage::unreached`) | — |
| F120 | The route table existed only as a debug log line | built | `AppHandle::handlers()` lists every mounted handler with its route and metadata (`crates/ulo/src/app/handle.rs:178-182`; `HandlerInfo::route`, `crates/ulo/src/transport/handler.rs:130`) | — |
| F243 | Every adapter-written error envelope outside routing guarded an impossible failure, carried a non-error, or belonged to core; invariant *an adapter never authors an error body* | built | No embedding crate writes a JSON or problem body (grep). Two residual host-authored refusals remain, both guarding a cross-version conversion that cannot fail: actix's empty `400` (`ulo-http-actix/src/service.rs:72-75`) and rocket's `Outcome::Error(Status::BadRequest)` (`handler.rs:57-58`, rendered by rocket's catcher) | Both bypass the error chain; if either became reachable, the answer would be the host's |
| F244 | actix's `http` 0.2 → 1 bridge substituted `GET` and `/` for an unparseable method or URI and dropped bad headers | built | `head()` answers `None` on any failure (`service.rs:90-103`) and the request is refused rather than rerouted; the doc states the invariant ("which actix parsed already and so never yields") | The invariant (both `http` versions implement RFC 9110's token grammar and RFC 3986) is now written; it is held by no test |

---

## E. Spec corrections (`HTTP_AUDIT.md` §6)

| ID | What | Status | Evidence at HEAD | Value to extract |
| --- | --- | --- | --- | --- |
| E1 | Two claims made before the specs were fetched, corrected: (a) a `Content-Length` on a 204 does not hang a conformant client; (b) RFC 9112 §6.2 does not forbid `Content-Length` on a 204 | built | `service.rs:556-558` cites RFC 9110 §8.6 for 1xx/204 and RFC 9110 §6.4.1 for the no-content rule | RFC 9112 §6.3 item 1: a 204 response "is always terminated by the first empty line after the header fields, regardless of the header fields present in the message". §6.2 forbids `Content-Length` only alongside `Transfer-Encoding`. What survives of the defect is a body reaching the wire as the start of the next message (W10) |

---

## F. Decisions (`HTTP_AUDIT.md` §7)

| ID | Decision | Status | Evidence at HEAD |
| --- | --- | --- | --- |
| D1 | Matching moves into core, in full; the report-the-miss-only alternative rejected ("the path is not the deliverable, the contract is") | built | T§3.2; `ulo-http` router shared by backend and embeddings |
| D2 | `%2F` inside a path parameter stays encoded; everything else decoded | superseded | Decoded inside the segment without splitting (`router/pattern.rs:8-10`, T§3.2:450); decided in `divergences/race2a-H.md` §1 on UTF-8 grounds |
| D3 | Status is `http::StatusCode`, re-exported | built | `response.rs:15`; `ulo_http::StatusCode` (`lib.rs:130`) |
| D4 | Response headers are `http::HeaderMap`; append vs insert chosen at the call site | built | `response.rs:15`, `:124-131` |
| D5 | One body ceiling, 2 MiB by default, with a knob, enforced where the body is collected | built | T§3.6 (`body_limit`, 2 MiB unset), `limits.rs:13-18` |
| D6 | RFC 9457 problem details replace the envelope: `type`, `title`, `status`, `detail`, `instance`; `details()` as top-level extension members | built (differently) | `render.rs:165-190`, `:209-218`: no `instance` member; `details` serialised as one nested `details` member rather than top-level members |
| D6b | `render_error` made reachable on HTTP, RPC and WebSocket (with S4) | superseded | See F87 |
| D6c | No feature flag emitting the legacy shape | dropped | No legacy shape exists to toggle |
| D7 | A bindless lifecycle, as a phase after the route table | built | `Embedded<A>` is "a `Server` binding nothing" (`crates/ulo-http/src/embed.rs:20`, `:641`); `AppService::call` is public; the crate's own tests drive it through a null backend (`crates/ulo-http/tests/common/mod.rs:1-3`) |
| D7b | Build the route table independent of any adapter | built | The router is built in `prepare` from mounted handlers (`router/mod.rs:76-88`) |
| D8 | An app-wide prefix named `use_global_http_prefix` | superseded | Per-controller `.at(prefix)` (T§2.5, X3) and `Embedded::nested_at` (`embed.rs:413`); no app-wide prefix on a backend |

**D2, value (the argument HEAD did not weigh).** A decoded separator turns a single-segment
parameter into a multi-segment one, and the common sink for a path parameter is a path join
(`root.join(param)`), so decoding `%2F` hands the caller a traversal primitive: `/files/..%2F..%2Fetc%2Fpasswd`
against `/files/{name}` captures `../../etc/passwd` as one `name`. The audit's cost note: axum, poem
and salvo decoded it, so leaving it encoded is the opposite of what an axum user expects, which is
why the rule belongs in one decoding rule rather than per adapter (now true). What HEAD recorded
instead (`race2a-H.md` §1, `transports/DIVERGENCES.md` T22): `PathParams` holds `String`s, `Routed`
has no error variant, and invalid UTF-8 is captured with U+FFFD rather than answered 404. That
covers UTF-8 and is silent on the separator. A `{*rest}` capture carries the same question for `..`
segments, since the raw remainder is captured decoded (`router/pattern.rs:101-107`).

**D3, value (rejected alternatives).** Keeping `u16` and fixing rocket alone leaves `.status(9999)`
compiling and rendering 500 everywhere. A ulo enum or newtype was rejected because status codes are
an IANA registry and RFC 9110 has a client treat an unrecognised code as `x00`, so a closed set
carries F70's defect, and a newtype needs `Display`, `FromStr`, serde and per-adapter conversions to
arrive at a worse `http::StatusCode`. The call-site count that made it cheap: 10 numeric
`.status(n)` calls, eight inside `http_response.rs` defining the named constructors; the migration
was 26 struct literals.

**D6, value.** RFC 9457 extension members are the RFC's own mechanism for additional fields; one
nested `details` object is also an extension member, so HEAD is conformant, but a client reading
`body.field_violations` finds it at `body.details` instead. `instance` (the request path) is the
member a log correlation reads. The trade the audit recorded and HEAD keeps: RFC 9457 is HTTP's
alone, so HTTP diverges from RPC and WebSocket, and that divergence is correct because a wire shape
is decided by the specification that owns it (`HTTP_AUDIT.md` §7 D6; ADR-0053 Roads not taken:
carrying problem details over a broker or a WebSocket frame "would be inventing a use the RFC does
not define … with a standard's name attached").

**D8, value.** The naming rule: a bare `set_global_prefix` reads as applying to every transport,
where a prefix is a URL concept; the `_http_` infix was the project's statement that a global
belongs to one transport, and `use_global_middleware` (HTTP-only by ADR-0007) broke it. Both names
are gone at HEAD.

---

## G. The pipeline gate's amendments (`HTTP_AUDIT.md` §10 preamble)

| ID | What | Status | Evidence at HEAD |
| --- | --- | --- | --- |
| G1 | Strike "a miss runs the global interceptors" from ADR-0052: an interceptor wraps a call and a miss is not one; a miss is offered to the global error handlers only | built | T§3.3: "on a miss only the global error handlers apply, since no handler matched" |
| G2 | `dispatch` absorbs the global chain, and the chain's failure reaches the error handlers (F125) | built | T§3.3, both sub-steps inside the error chain |

---

## H. Out of scope (`HTTP_AUDIT.md` §9)

| ID | What | Status | Evidence at HEAD | Value to extract |
| --- | --- | --- | --- | --- |
| O1 | Conditional requests and caching (RFC 9111): no `ETag`, `Last-Modified`, `If-None-Match` | open (scope, unfiled) | `grep -rli 'etag\|if-none-match\|last-modified' crates/ulo-http/src` finds nothing | A scope decision, not a defect |
| O2 | gRPC's half of F105: tonic owns method routing; a fallback service on tonic's `Routes` was the narrow remedy, unverified for tonic 0.14 | UNVERIFIED | The gRPC server is rebuilt on `ulo-server-hyper` (T§6.2); belongs to the gRPC subject | — |
| O3 | HTTP/2, HTTP/3, compression, static files, range requests, trailers untested | open (scope) | HTTP/2 built (h2c, `max_concurrent_streams`, GOAWAY at drain, T§3.6); HTTP/3, compression, static files, ranges not present | — |
| O4 | A CORS preflight advertises methods from static configuration while the route table knows the answer, so a preflight can approve a method the request then 405s | open (unfiled) | `Cors::preflight` writes `Access-Control-Allow-Methods` from `self.methods` (`cors.rs:144-146`); preflights never reach routing (T§3.4) | With a core-owned table the route's `Allow` is computed in `prepare` (`router/mod.rs:282-293`) and could be the default for the preflight's method list |
| O5 | `STANDARDS_AUDIT.md` HTTP items not audited: RFC 7578 beyond missing constraints, RFC 6265 beyond the absent surface | open | Cookies absent (F115); multipart per-field limits absent (F116) | — |

---

## I. Build sequence rows (`HTTP_AUDIT.md` §10, `BUILD_SEQUENCE.md` rows 76–106, 131)

| Row | What | Status | Evidence at HEAD |
| --- | --- | --- | --- |
| 76 HTTP-1 | `AdapterCapabilities`; the capability ADR (S10) | superseded | `BackendLimits` / `EmbedLimits` (H4) |
| 79 HTTP-2 | Core route table; duplicate detection; `routes()` read (S12) | built | F4, F120 rows; own matcher instead of `matchit` |
| 80 HTTP-Q | One query parser for the framework (`serde_html_form`, S1) | open | Own parser without sequences (F117) |
| 81 HTTP-3 | `dispatch`, `Dispatched`, 404/405/`Allow`/`HEAD`/`OPTIONS`/`//`/decoding; `Unrouted` to the global handlers; F125 | built | W1–W4, F105, G1, G2; decoding differs (D2) |
| 86 HTTP-4 | Migrate axum | superseded | Rewritten as the tower embedding (`crates/ulo-http-axum`) |
| 83 HTTP-5 | Migrate poem | superseded | Rewritten as a native embedding (`crates/ulo-http-poem/src/endpoint.rs`) |
| 84 HTTP-6 | Migrate salvo | superseded | Rewritten as a native embedding (`crates/ulo-http-salvo/src/handler.rs`) |
| 87 HTTP-7 | Migrate actix; infallible conversion; count and log dropped headers (F244) | superseded | Rewritten (`crates/ulo-http-actix`); conversion refuses rather than counts (F244 row) |
| 85 HTTP-8 | Migrate rocket; the WS marker dies | superseded | Rewritten (`crates/ulo-http-rocket`); H8 |
| 88 HTTP-9 | Remove `register_route`, `register_ws_route`, `RequestHandler` | superseded | None of the three exists |
| 102 HTTP-10 | ADR-0054: `StatusCode`, `HeaderMap` | built | D3, D4 |
| 103 HTTP-11 | ADR-0053: problem details | built | D6 |
| 104 HTTP-12 | `Vary: Origin` unconditionally; a cookie surface | open | `Vary` built (F109); cookies absent (F115) |
| 105 HTTP-13 | 2 MiB ceiling, declared per adapter; multipart from it | built | D5; per-field multipart open (F116) |
| 106 HTTP-14 | `use_global_http_prefix` | superseded | D8 |
| 131 D3-ii-HTTP | A running HTTP handler learns its caller is gone: a drop guard at the route wrapper, disarmed at a clean return; detached work holding a token clone stops | open | Below |
| S1 | Ruling 2026-09-24: `serde_html_form` 0.4.1 for every query parser | superseded | Not adopted; own parser (`extract/de.rs`) |
| S4 | Ruling: `render_error` reachable on HTTP, RPC, WebSocket in one PR | superseded | F87 row |
| S6 | Amendments to ADRs 0052/0053 while `proposed` | dropped | The ADRs were never brought to master (HO2) |
| S8 | `HTTP-3` and `WS-21` as two PRs | superseded | Upgrades through `UpgradeHandler` (T§3.5) |
| S10 | One capability record across HTTP, WS, RPC, GraphQL | superseded | Per-transport limits structs; HTTP's in `backend.rs` / `embed.rs` |
| S12 | HTTP-2 ships `routes()` and closes F120 | built | F120 row |

**Row 131, value.** The gRPC transport fires `ClientCancelled` when a call's future is dropped
before it answers (T§2.6, `CancelReason` list). HTTP fires `Disconnected` only through the response
body (`ExecBody`'s `Drop`, `service.rs:948-955`), so a buffered handler still computing when the
peer closes has no signal: the service holds no guard on the pipeline future, and dropping the
`Execution` does not cancel (`execution/mod.rs:412-417`). The service doc's sentence "A peer leaving
before the head is written is a connection failure, not an incomplete body" (`service.rs:75-76`)
describes the outcome without naming the consequence for a handler. This is the HTTP analogue of
F245 (RPC's unary call aborted without firing its token). Whether hyper 1.11 drops the service
future on an HTTP/1.1 disconnect while the service is pending is the run that decides how often it
occurs. The test the row named: a handler that spawns work holding `cx.exec().cancelled()`, a
client that closes before the response, and an assertion that the work observes cancellation.

---

## J. Handoff, artifacts and process (`HTTP_BUILD_HANDOFF.md`, `artifacts/`)

| ID | What | Status | Evidence at HEAD | Value to extract |
| --- | --- | --- | --- | --- |
| HO1 | The divergence gate, the Law review, the per-PR checklist, merge with `--merge`, bookkeeping into `DOCS_AUDIT.md` §18 | dropped | The build never ran; the process rules live in memory and `~/.claude/` | — |
| HO2 | ADRs 0052–0054 on branch `docs/http-audit-adrs` (`9d3fb9c2`) | open | No branch holds the commit; `git fsck --unreachable --no-reflogs` lists `9d3fb9c22fc6…`; `docs/adr/` at HEAD jumps from 0051 to 0055 | Preserve `docs/adr/0052-the-framework-owns-the-http-route-table.md`, `0053-the-http-error-body-is-problem-details.md`, `0054-a-response-carries-the-ecosystems-types.md` from that commit before a gc. Their Roads-not-taken sections hold the arguments in §A H2/H3, §F D3/D6 |
| HO3 | Divergence-gate item: "the spike covered poem and salvo, rocket already ships the shape; axum and actix are inferred, not probed" | built | axum hands `OnUpgrade` over (`ulo-http-axum/src/lib.rs:60`) and runs `upgrade_echoes_a_frame`; actix declares no upgrades (`ulo-http-actix/src/lib.rs:29`, `EmbedLimits::upgrades`), refused in `prepare` | — |
| P1 | `spike-dispatch-point-upgrade.patch` | built | See H7 | What it proves: a same-port upgrade can be decided by code that sees only the framework's request and performed afterwards from the retained native request, with no native route mounted. The claim holds at HEAD in the `Request.upgrade` shape |

---

## K. What the audit found about each old adapter that still teaches

| Adapter (at `654c7183`) | What the audit found | At HEAD | What it teaches |
| --- | --- | --- | --- |
| axum 0.8 | Native router gave `HEAD` and `Allow: GET,HEAD` free; a duplicate route panicked out of `bind` (F4); `insert` flattened repeated headers (F108); `adapt_request`'s `Result` was decorative, its `Err` arms dead (F243) | Tower embedding (`fw_http::embed::Service<Axum>`), `ConnInfo` from a layer of its own, upgrades via `take_upgrade` | A native router's free behaviour is a behaviour the other hosts lack; a `Result` on an infallible conversion invents error paths no test reaches |
| actix-web 4 | The second engine (actix-http, `!Send` per-worker runtime, a oneshot bridge for the global chain); request bodies buffered at 256 KiB (F27); `//probe` answered 405; a 204 body reached the wire (F110, response splitting); a CRLF header failed the whole response with an empty 500; the `http` 0.2 bridge substituted `GET /` (F244) | Native embedding; payload pumped through a bounded channel; conversion refuses; no upgrades; no GOAWAY on an HTTP/2 drain, the connection reset at `shutdown_timeout` (`lib.rs:42-47`) | Two `http` versions in one process need a stated invariant at the bridge; a 204 body is the one W-row with a security consequence and the one still to re-probe here (W10) |
| salvo 0.92 | Kept a second route table and hand-written `path_matches` for 405 because "salvo's router cannot distinguish a method mismatch from no match" (ADR-0007); `//probe` reached the handler; the spike passed | Native embedding over `salvo::Handler`; `OnUpgrade` taken from extensions | A host that cannot tell 404 from 405 forces a second table; owning the table removes the class |
| poem 3.1 | The only adapter that appended headers (both `Set-Cookie` arrived); 405 without `Allow` (RFC 9110 §15.5.6 MUST) and a text body; the spike passed | Native embedding over `poem::Endpoint` | One of five being right about a multimap showed the core model, not the adapters, had to decide |
| rocket 0.5 | No usable native routing: dispatch behind per-method catch-all mounts with its own `match_route` (accepting legacy `:p` and `*tail`); `Status::from_code` turned `599` into 500 (F114); decoded nothing (`a%20b`); a CRLF header closed the connection with zero bytes; an unknown method answered by rocket's parser with HTML 400; refused a pre-bound listener; the WS upgrade signalled through a magic header | Native embedding, still nine catch-all mounts (`handler.rs:27-36`); `Status::new`; upgrades through `IoHandler`; body buffered under the app's limit; every request presented as HTTP/1.1 (F382) | A parser-level refusal is below any SPI (W5); rocket 0.5 runs hyper 0.14, a third engine (H10); a host that drops the protocol version makes version-dependent rules unreachable (F382) |

---

## Worth discussing with the designer

1. **Should `%2F` inside a path parameter decode?** HEAD decodes it (`router/pattern.rs:8-10`); the
   audit decided it stays encoded on the traversal argument (§F D2), and HEAD's recorded reasoning
   addresses UTF-8 only. Recommendation: keep `%2F` encoded inside a `{param}` capture (and state
   what a `{*rest}` capture does with `..`), or record the traversal argument as weighed and
   rejected in T§3.2. Either way the conformance scenario `path_parameter_percent_decoded` pins the
   choice.
2. **Should an HTTP handler learn that its caller left before the response head?** (§I row 131.)
   gRPC does (`ClientCancelled` on a dropped call future); HTTP does not. Recommendation: a guard on
   the pipeline future in `AppService::call` firing `Disconnected` when dropped before it answers,
   disarmed at return, matching gRPC; probe hyper's drop behaviour first.
3. **Should a path-scoped pre-dispatch entry run on a miss under its scope?** (F118.) At HEAD an
   `apply_for::<ApiKeyAuth>(["/admin/*"])` runs only when a route matched, so `/admin/anything`
   answers 404 to an anonymous caller where an existing admin route answers 401. Recommendation:
   run a scoped entry whose scope covers the request path on a miss too, in the unscoped sub-step's
   position, or document that an entry guarding a prefix against enumeration must be unscoped.
4. **Should the server refuse an HTTP/1.1 request with no `Host` (RFC 9112 §3.2 MUST) and a request
   carrying both `Content-Length` and `Transfer-Encoding` (RFC 9112 §6.3, "ought to be handled as an
   error")?** Recommendation: probe hyper 1.11 and actix-http 3.18 first (§C); if either accepts,
   one check in `AppService::call` applies to every host.
5. **405 or 501 for an unrecognised method?** RFC 9110 §9.1 says SHOULD 501. Recommendation: 501
   for a method token the framework does not recognise as standard (`http::Method` extension
   methods with no handler anywhere in the table), 405 for a standard method the matched route
   lacks; rocket's parser answers first regardless and is recorded as a host limit.
6. **Problem-details shape: `instance`, and `details` as top-level members?** (§F D6.)
   Recommendation: add `instance` from the request path; keep `details` nested only if the gRPC
   one-to-one mapping (T§2.4) needs a single member, and say so in T§2.4.
7. **Should the §2 wire table become conformance scenarios?** (§B W14.) Recommendation: yes; the
   embeddings exist to be held to the reference, and these are the measured disagreements.
8. **Should a 204/304 body be replaced with an empty one in the service** rather than handed to the
   host marked unwritten (§B W10)? Recommendation: replace it in `AppService::call`, so no host's
   encoder decides; the `warn` stays.

## Not verifiable here

- **W10** — whether hyper 1.11.1 and actix-http 3.18.12 write a 204's known-length or streaming
  body; needs the pipelined `204`-then-`GET` probe per host.
- **I1, I2** — `Host`-less HTTP/1.1 requests and `Content-Length` + `Transfer-Encoding` on hyper 1
  and actix-http; needs raw-TCP probes.
- **Row 131** — whether hyper drops a pending service future when an HTTP/1.1 peer closes, and on
  HTTP/2 at `RST_STREAM`; needs a probe with a spawned task holding a cancellation clone.
- **H14** — the per-request routing cost now that every host routes in core; needs a benchmark.
- **O2** — tonic-era gRPC miss reporting; belongs to the gRPC subject.
- **W5 (rocket half)** — the exact answer rocket 0.5.1 gives `PROPFIND` today (read as rocket's own
  400 from the nine-method mount; not run).
