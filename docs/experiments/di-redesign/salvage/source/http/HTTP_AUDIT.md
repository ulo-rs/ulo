# HTTP audit — the adapter seam, and what the wire says

Opened 2026-09-20. Scope set by the user: an HTTP pass in the shape of the SSE one, folding in
*"shouldn't there be a standardized SPI that the adapter and framework can use to communicate
better… hey I found no route… hey here is a request take care of it… hey here's the response"*,
and asking whether the earlier finding that ulo cannot serve without a socket bears on it. It does:
§8.

**Where the findings live.** `FRAMEWORK_GAPS.md`, F105–F120. This memo is the plan and does not
restate them.

**Not a build.** Nothing is committed. One throwaway branch existed for the §1 spike and was
deleted after it answered; master is untouched at `654c7183`. Work this recommends branches per
section.

## Confidence marking

- **Probed** — run this session against master `654c7183`, five adapters, raw TCP so the bytes are
  the evidence rather than a client's reading of them. Every probe was also falsified — disabled and
  re-run to confirm it was measuring what it claimed. The probe files and the spike branch were
  removed; the tree is clean.
- **Read** — read off the source, cited by file and line.
- **Spec** — the specification's own words, fetched and quoted rather than recalled. Two claims
  written from memory did not survive that check and are corrected in §6.
- **Unverified** — believed, not established. Marked inline, and not to be acted on before
  falsification.

## The shape of it

One structural finding and a field of consequences. §1 is the seam: HTTP is the only transport
whose adapter owns routing, and almost everything in §2–§6 follows from that — the divergences are
what five independent implementations of an unspecified behaviour look like. §4 and §5 are
independent of it and can be taken in any order. §7 is the section to answer; everything above it
is the evidence for it.

---

## 1. The seam

**The correlation.** (Read) Four transports, two answers to the question *who decides which handler
runs*:

| transport | what the framework hands the adapter | who matches | miss reportable | copies of the envelope |
| --- | --- | --- | --- | --- |
| RPC | the pattern list and one callback (`rpc/adapter.rs:64-72`) | the framework | yes — `Unrouted`, `application.rs:1044` | one, in `rpc::wire` |
| WebSocket | a path and one callback per gateway (`ws/adapter.rs:116-121`) | the adapter matches the connection, the framework matches the event | yes, for the event — `__ws.rs:63` | one |
| HTTP | one handler per route (`http/adapter.rs:57-62`) | the adapter | **no** | **six** |
| gRPC | service sources (`grpc/adapter.rs:36-39`) | tonic | **no** | tonic's |

The two transports that kept the matching decision are the two that can report a miss. That is not
a coincidence: reporting it requires having made it.

**The rule this suggests**, stated as a test so the next transport is decided rather than argued:
*an adapter routes down to the smallest unit the framework can name, and no further.* WebSocket is
the clean case — the framework names an event, so the adapter routes the connection and stops. The
framework names an HTTP route, so on HTTP the adapter should route nothing.

**Why HTTP went the other way**, fairly: the native routers are good and give axum and poem correct
`HEAD` for free; a same-port upgrade needs the adapter's native request type; and HTTP was written
first, before there was a second transport to be consistent with (`AUDIT3_DISPATCH.md` §1).

**What measurement says about the first reason.** (Read) Three of the five adapters already match
paths themselves, in adapter-local code: rocket does all of its dispatch that way behind per-method
catch-all mounts (`rocket_adapter.rs:67`, `:255-300`), salvo and actix keep a second route table to
answer 405 (`salvo_adapter.rs:284`, `actix_adapter.rs:175`). So the native router is carrying the
work on two adapters. On three it is carrying part of it, beside a hand-written matcher nobody
specified — and §2 shows the three disagree.

### What ADR-0007 already decided, and what has changed since

This was considered in July 2026 and rejected. ADR-0007's own words, under *Considered and
rejected*: **"A ulo-owned router (`matchit`) in the serve path. It would make the five native
routers dumb byte-pumps and re-implement path matching the hosts already do well. Nest never does
this; the adapter contract doesn't need it."** ADR-0052 has to answer that rather than walk past it.

Three answers, in increasing weight:

1. **The same ADR kept the door open twice.** The rejection continues: *"`matchit` remains a
   legitimate private mechanism for an adapter whose host offers no anchor point… and for a future
   adapter-independent portless handler — both out of scope here."* Its Consequences section adds
   that binding the socket is *"the natural seam for portless dispatch (in-process testing,
   serverless) if that lands later."* So this is the consequence ADR-0007 flagged, not a reversal of
   it.

2. **"Nest never does this" is the weakest of the three reasons, and the project has already ruled
   on that class of argument.** Nest delegates matching to two Node HTTP servers with compatible
   semantics; ulo delegates to five Rust servers with five. F65's finding stands behind this: an
   ecosystem's popular framework is not a standard, and copying NestJS is not a reason on its own.

3. **"The hosts do it well" is the load-bearing reason, and §2 falsifies it.** The evidence did not
   exist in July 2026 — no probe had been run. The hosts do path matching well *individually*; what
   they do not do is agree. `//probe` draws three different answers, `HEAD` on a GET route three,
   `%2F` in a parameter three. Two of the five hosts are not doing the work at all: salvo and actix
   keep a second route table because, in ADR-0007's own account, *"salvo's router cannot distinguish
   a method mismatch from no match"*, and rocket has no usable native routing whatsoever.

**And the precedent runs the other way.** Three ADRs have already moved a piece of routing into core
because the adapters could not agree on it. ADR-0011 took trailing-slash canonicalisation — *"The
fix cannot live in the adapters. Five routers have five strictness models."* ADR-0012 took the
parameter syntax, having counted *"five adapters times two syntaxes… ten translation points to keep
in sync"*, and recorded that `{id}` was *"invisible to the salvo/actix 405 fallbacks."* ADR-0007
took the middleware anchor. ADR-0052 takes what those three were working around.

### The proposed SPI

```rust
pub trait HttpAdapter: Send + Sync + 'static {
    fn capabilities(&self) -> AdapterCapabilities { AdapterCapabilities::default() }
    async fn into_lifecycle(self: Box<Self>, target: BindTarget, ctx: ServeContext)
        -> AdapterResult<HttpLifecycleHandle>;
}
```

Two methods where there are four: `register_route`, `register_ws_route` and `streams_responses` go.
`ServeContext` carries the entry point, replacing `execute`:

```rust
impl ServeContext {
    pub async fn dispatch(&self, req: HttpRequest) -> Dispatched;
}

#[non_exhaustive]
pub enum Dispatched {
    Respond(HttpResponse),
    Upgrade(Arc<WsConnectionCallbacks>),
}
```

An adapter's work per request becomes: build the `HttpRequest`, call `dispatch`, then write the
response or perform its native upgrade with the callbacks it was handed. Canonicalisation, the
global chain, matching, 404, 405, `Allow`, `HEAD`, `OPTIONS` and the envelope move inside
`dispatch`, written once. This is the RPC trait's shape, reached for HTTP.

**On the negotiation.** The exchange in the ask — *I have a request / I cannot take that / give me
the pipeline / here is the response* — is right about the seam and one step longer than it needs to
be. The only thing an adapter can refuse is a capability, and a capability does not
change between requests, so the refusal is static and belongs in a declaration. What is left per
request is the framework answering *what to do with this*, which is `Dispatched`. The round trip
collapses to one call and one enum, and nothing is lost: the case the negotiation was invented for
— a same-port WebSocket upgrade, where the answer is not a response — is the second variant.

**`AdapterCapabilities`** is the static half, read once at `use_http_adapter`:

```rust
#[non_exhaustive]
pub struct AdapterCapabilities {
    pub streams_responses: bool,          // today: a defaulted trait method (F103)
    pub same_port_upgrade: bool,          // today: an Err from register_ws_route
    pub adopts_listener: bool,            // today: rocket's Err at bind (ADR-0013)
    pub max_request_body: Option<usize>,  // today: discovered at runtime (F27, F112)
}
```

Three refusal mechanisms become one table, checked in one place, before a socket opens. Adding a
field later is not a breaking change, which is what the defaulted-method approach was buying and
paying a method for.

**The matcher.** Core needs one. `matchit` 0.8 is the candidate: it is already in the lock file
under axum, it is a data structure with no runtime dependency — which
[[feedback-keep-runtime-out-of-core]] requires — and its parameter syntax is `{param}`, which is
ulo's canonical spelling since ADR-0012 and why the axum adapter mounts paths unchanged. Adopting
it keeps axum's present matching exactly and deletes three hand-written matchers.
**Unverified:** whether it accepts everything the five adapters accept today — rocket's
`match_route` also takes the legacy `:p` and `*tail` spellings, which ADR-0012 already refuses at
the macro, so the question is whether anything else reaches it.

**What it closes.** F105, F106, F107, F111, the adapter half of F110, and the enforcement point
F112 needs. F4 becomes a check over one table before bind rather than an axum panic through it.
F103 and F104 both dissolve into the capability table. The parked `set_global_prefix` becomes a join over
one table rather than an argument about where registration happens, and route introspection becomes
possible at all.

**What it costs.** Five adapter crates rewritten at their routing seam — rocket's shrinks, salvo's
and actix's lose a fallback handler and a matcher, axum's and poem's lose per-route registration and
gain one fallback service. One extra match per request on axum and poem, unmeasured.

**On timing, which is the sharpest argument for doing it now.** Nothing is published to crates.io
yet. The SPI break therefore costs nothing today and becomes a 1.0 break afterwards, so this change
belongs before the first publish rather than in the queue behind it.

### The spike — **run 2026-09-20, passed on both**

The design stood or fell on same-port upgrade: each adapter matches the WS path itself and mounts a
native route holding an upgrade extractor, and under this SPI it has to upgrade at its dispatch
point instead, on a request the framework already decided about. Poem and salvo bind the upgrade to
an extractor on the matched route and were the likeliest to fail.

Both were patched on a throwaway branch at `654c7183`: the WS route was **not** mounted on the
native router at all, a `SpikeDispatched::{Respond, Upgrade}` decision was taken by a function
seeing only the ulo `HttpRequest`, and the upgrade was performed from the native request shell
retained across that call. `ws_same_port_upgrade_and_echo` passes on both, with the rest of each
adapter's suite.

**Falsified before being believed.** Disabling the decision (`if false && …`) makes the same test
fail with `404 Cannot GET /ws` on both adapters, so nothing else was serving the upgrade and the
passing run was the spike path. The branch is deleted; the diff is kept in this session's
scratchpad as `spike-dispatch-point-upgrade.patch`.

**Why it worked, which is the part to keep.** Both adapters already retain the native request across
the whole global chain, for the reason ADR-0007 recorded — poem's comment says the shell "keeps
state http parts cannot carry — the WebSocket upgrade slot", and salvo's `mem::take` moves the
upgrade state with it. And both already round-trip the upgrade's 101 response through ulo's
`HttpResponse`, which keeps only status, headers and body. So the upgrade is driven entirely by the
*request* slot and the handshake response is ordinary data. `Dispatched::Upgrade` needs no
out-of-band channel.

Rocket is the third witness and needed no spike: it already ships this design. Having no usable
native routing, its handler decides the upgrade from the ulo request and signals it to the outer
layer through a magic response header (`x-ulo-rocket-ws-route`, `rocket_adapter.rs:204-217`). That
is `Dispatched::Upgrade` hand-rolled, out of band, because the return type could not express it.

**What stays the adapter's**, so the line is written down: accepting connections, parsing the
message, body framing, `Expect: 100-continue`, TLS, HTTP/2, the upgrade handshake, and writing the
response. Some refusals live below this line and no SPI recovers them — rocket answers its own HTML
400 to an unrecognised method inside its parser, before any ulo code runs (§2).

### What an adapter is for afterwards

The question this raises — *if core matches, what do the native crates still buy?* — has a sharper
answer than it looks, and it should be settled in ADR-0052 rather than left as a feeling.

**Four of the five are the same protocol stack.** axum, poem, salvo and rocket all sit on hyper;
for accept, TLS and the HTTP protocol they are the same code. actix is the second
implementation — actix-http, with its own parser and a per-worker `!Send` runtime, which is why the
actix adapter needed a oneshot bridge for the global chain (ADR-0007) and why it buffers request
bodies. There were never five HTTP implementations here. There are two, wrapped five ways.

So after this change the reason to keep five is not implementation, it is **interop**: a tower
layer, an actix middleware crate, a rocket fairing, or an existing application on one of those
servers that wants ulo's DI and pipeline inside it. That is a better reason than five ways to match
a path, and it is the one the docs should give.

**The move it opens: `ulo-http-hyper`**, a first-party adapter with no framework in between, as the
reference implementation and eventual default. Today no fix in §2–§6 can be made at the source,
because five crates write the response; every fix is five fixes or a conformance test that pins
disagreement. With core owning the decisions and one adapter owning the wire, the conformance suites
stop asking whether the five agree and start asking whether each matches the reference. What is not
free: hyper supplies the protocol and not the serving policy — accept-loop error handling, header
limits, timeouts, graceful-shutdown semantics — which axum and poem decided on the caller's behalf.
`hyper-util`'s `auto::Builder` and `graceful` narrow that gap and do not close it.

**And the five are a conformance asset.** §2's table exists only because five independent
implementations exist; one implementation would have hidden every defect in it. That argues for
keeping them and adding the reference, not for culling.

---

## 2. One program, five wires

(Probed at `654c7183`. One module registering `GET /probe`, `GET`+`POST /both`, `GET /seg/{id}`.)

| | axum | actix | salvo | poem | rocket |
| --- | --- | --- | --- | --- | --- |
| `GET //probe` | 404 | 405 | **200** | 404 | **200** |
| `HEAD /probe` | 200, no body | **405** | **405** | 200, no body | **405** |
| `OPTIONS /probe` | 405 | 405 | 405 | 405 | 405 |
| `POST /probe` body | empty | envelope | envelope | `method not allowed`, text | envelope |
| `POST /probe` `Allow` | `GET,HEAD` | `GET` | `GET` | **absent** | `GET` |
| `PROPFIND /probe` | 405 | 405 | 405 | 405 | **400, rocket's HTML** |
| `/seg/a%2Fb` | `a/b` | `a%2Fb` | `a/b` | `a/b` | `a%2Fb` |
| `/seg/a%20b` | `a b` | `a b` | `a b` | `a b` | **`a%20b`** |
| two `Set-Cookie` | last only | last only | last only | **both** | last only |
| 204 with a body | `Content-Length: 18` | **body on the wire** | clean | clean | `Content-Length: 18` |
| status `599` | `599` | `599` | `599` | `599` | **500** |
| header value with CRLF | dropped | **500** | dropped | dropped | **0 bytes** |
| 8 MiB body | 200 | **413** | 200 | 200 | 200 |

Filed as F106 (routing), F107 (methods), F108 (headers), F110 (204), F111 (decoding), F112 (body),
F113 (invalid header), F114 (status).

**The half that agrees**, kept in view because it is what says the rest is a defect rather than a
fact of life: the 404 envelope is byte-identical in shape on all five; a duplicate `Content-Length`
is 400 on all five; absolute-form request lines, `Expect: 100-continue`, chunked request bodies and
HTTP/1.0 all work on all five; `/./probe` and `/x/../probe` are 404 on all five; and no adapter let
the injected header line reach the wire. Where the answer is the underlying server's, the five
agree. Where it is the adapter crate's, they do not.

---

## 3. What is inherited, and not ulo's

Named so it is not rediscovered as a ulo defect.

**No `Host` header on an HTTP/1.1 request is answered 200 by all five.** (Probed/Spec) RFC 9112
§3.2: *"A server MUST respond with a 400 (Bad Request) status code to any HTTP/1.1 request message
that lacks a Host header field."* The check belongs to hyper and actix-http, below the SPI. It is
enforceable in the global chain in four lines; whether to is D7.

**`Content-Length` and `Transfer-Encoding` on one request is accepted by four and refused by
actix.** (Probed/Spec) RFC 9112 §6.3: *"If a message is received with both a Transfer-Encoding and
a Content-Length header field, the Transfer-Encoding overrides the Content-Length. Such a message
might indicate an attempt to perform request smuggling… and ought to be handled as an error."* Four
adapters take the Transfer-Encoding and answer 200, which the same sentence permits and the clause
after it discourages. With a dispatch entry (§1) the stricter reading becomes one check in one
place; without one it is five.

---

## 4. The response header model — F108, F109, F115

`HttpResponse.headers` is a `Vec` and `HttpResponseBuilder::header` pushes, so core's model admits
repeats. Four of five adapters convert it with `insert`. RFC 9110 §5.3 on `Set-Cookie`: *"Since it
cannot be combined into a single field value, recipients ought to handle 'Set-Cookie' as a special
case."* Two cookies need two field lines and four adapters cannot produce them.

The live victim is not cookies. `CorsMiddleware` pushes `Vary: Origin` after the handler has run, so
a handler's own `Vary: Accept-Encoding` is replaced on four adapters and joined on one — probed. And
when a request carries no `Origin` the middleware returns before adding `Vary` at all, so a shared
cache stores that response with no secondary key and reuses it for a cross-origin request that
needed `Access-Control-Allow-Origin`; RFC 9111 §4.1 is the mechanism. The framework's own middleware
depends on append semantics that one adapter of five provides.

There is no cookie API (F115). Until the header model is settled, there cannot be a correct one.

---

## 5. The request body has no ceiling — F112, F116

Core's body extractors collect without a limit, and `HttpAdapter` has no knob for one — its doc
argues the absence of a concurrency knob at length and does not reach body size. The effective
ceiling is 256 KiB, 32 MiB or unbounded depending on which adapter is linked, and on the three
unbounded ones a single request sizes the heap. `Multipart` compounds it: the reader is built with
`multer`'s default constraints, which set no per-field or total limit, and `Field::bytes()` collects
a part whole.

This is the clearest case of the §1 thesis away from routing: a policy that has to be uniform is
being set in the one place that cannot be uniform. F27 records the actix end of it as a cap too
small to configure; the other end is three adapters with nothing.

---

## 6. What the framework never decided — F110, F113, F114

Three values core will construct and hand over without an opinion: a 204 carrying a body, a status
outside the registered range, and a header value the wire cannot carry. Each is rendered five ways
because five adapters each decided separately, and in none of the three does the application learn
what happened to its value.

**Two corrections to claims made before the specs were fetched**, recorded because the confidence
marking exists to make exactly this tractable:

- *"axum and rocket send an illegal `Content-Length` on a 204, and a strict client hangs."* Wrong on
  the consequence. RFC 9112 §6.3 item 1: a 204 response *"is always terminated by the first empty
  line after the header fields, regardless of the header fields present in the message."* A
  conformant recipient does not wait for those 18 bytes. What survives is that actix's body reaches
  the wire and is read as the start of the next message — the pipeline probe shows
  `…\r\n\r\nLEAKHTTP/1.1 200 OK…` — and that one value renders five ways.
- *"RFC 9112 §6.2 forbids `Content-Length` on a 204."* §6.2 forbids it only alongside
  `Transfer-Encoding`. The framing rule is §6.3 item 1, quoted above.

---

## 7. Decisions that are the user's

**D1 — does matching move into core? — DECIDED 2026-09-20: yes, the full move.** The alternative
was the report alone — keep the seam and let `RequestHandler` answer "no match" — which closes F105
and leaves F106, F107 and F111 open, removing no hand-written matcher. Decided against on the user's
terms: the path is not the deliverable, the contract is. The upgrade spike that gated it has since
passed on poem and salvo (§1). **This decision is ADR-0052**, which supersedes ADR-0009's
registration surface and answers ADR-0007's rejection of a ulo-owned router.

**D2 — `%2F` inside a path parameter — DECIDED 2026-09-20: left encoded.** A decoded separator
turns a single-segment parameter into a multi-segment one and the common sink is a path join, so
the decode is a traversal primitive handed to the caller. Everything else in a captured segment is
decoded. Cost: axum, poem and salvo change behaviour, and it is the opposite of what an axum user
expects — which is why it belongs in the decoding rule §1 writes down rather than in each adapter.
Closes the ulo half of F111.

**D3 — what type is a status? — DECIDED 2026-09-20: `http::StatusCode`, re-exported as
`ulo::http::StatusCode`.** `from_u16` admits 100–999, which is exactly RFC 9110's three-digit range,
so `599` and `299` stay legal and `9999` stops being constructible — F114 closes by construction
rather than by five adapters agreeing. Rocket moves off `Status::from_code` to `Status::new`
regardless, since `from_code` answers `None` for a code it does not know. Actix converts through
`from_u16(code.as_u16())` because actix-http pins `http` 0.2 against ulo's 1.4, which is what it
already does with a `u16`.

Rejected: keeping `u16` and fixing rocket alone, which leaves `.status(9999)` compiling and then
rendering 500 everywhere with nothing said — F114's shape, agreed upon rather than removed. Also
rejected: a ulo enum or newtype. Status codes are an IANA registry and RFC 9110 has a client treat
an unrecognised code as `x00`, so a closed set carries F70's defect; a newtype would need `Display`,
`FromStr`, serde and a conversion per adapter, arriving at a worse `http::StatusCode`.

The eight named constructors are unchanged and stay the ergonomic path, and the field stays `pub` —
with a typed status there is no invariant left to protect.

**What it costs at call sites, counted rather than estimated.** The tree holds 10 numeric
`.status(n)` calls and **eight of them are inside `http_response.rs` itself**, defining the named
constructors; the other two are in integration tests. Every other call site in the repo already
writes `HttpResponse::ok()` or `::not_found()`. So a typed status is close to free where responses
are built, and the migration is the 26 `status: <n>` struct literals: 10 in the two GraphQL
controllers, 9 emitted from `quote!` inside `ulo-macros` (which name a path anyway, so
`::ulo::http::StatusCode::BAD_REQUEST` costs nothing new), and 7 across core and the actix adapter.

The dynamic case improves rather than degrades — a proxy forwarding an upstream status gains a way
to say the number was not one (`StatusCode::from_u16(n)?`), where today it becomes 500 on five adapters with nothing said.

**D4 — response headers — DECIDED 2026-09-20: `http::HeaderMap`, which dissolves the question.**
"List or map" was a false dichotomy: `HeaderMap` is a multimap, so `append` adds a field line and
`insert` replaces one, and the choice is made at the call site rather than by the adapter. It is
the same move as D3 and pays more — the adapters' conversion becomes a move instead of a per-pair
re-parse, and `HeaderName`/`HeaderValue` validate at construction, which closes F113 in the same
stroke. The fallible path takes the shape this project already settled on gRPC:
`set_header(&mut self, key, value) -> Result<(), InvalidHeader>`
(`crates/ulo/src/grpc/runtime.rs:242`) — the same API across two transports is legitimate in a way
the same *wire shape* across two transports is not.

Ordering: F109 and F115 are not defects that can be fixed before this lands. Pushing `Vary: Origin`
unconditionally is only a fix where pushing appends, and a cookie API that can emit one
`Set-Cookie` is not one. Recorded in ADR-0054 with D3.

**D5 — one body ceiling — DECIDED 2026-09-20: 2 MiB by default, with a knob.** Nothing else in the
framework is unbounded, and neither shipped value is a natural pick — 256 KiB is the smallest and
32 MiB the largest. 2 MiB is axum's own `Json` default and therefore the number most Rust HTTP
users already assume. Enforced where the body is collected so it holds on every adapter, and
declared per adapter through §1's capability table so a lower native cap is stated rather than
discovered. Closes F112 and gives F27 its answer; F116's multipart constraints derive from the same
number.

**D6 — does RFC 9457 `problem+json` replace the envelope? — DECIDED 2026-09-20: yes, in the change
that collapses the six copies.** Closes F65, and F69 falls out of it. The shape:

```json
{
  "type":     "about:blank",
  "title":    "Unprocessable Content",
  "status":   422,
  "detail":   "name must not be empty",
  "instance": "/users"
}
```

served as `application/problem+json`. `title` is the RFC 9110 registered phrase, which is F69's
422 correction arriving as a consequence. `Error::message()` becomes `detail`, `Error::kind()`
decides `status`, and `Error::details()` becomes top-level **extension members** — the RFC's own
mechanism, and better than nesting them. `type` defaults to `about:blank` for the case where only
the status carries meaning, leaving room for an application to name a problem type later.

**The trade, recorded rather than hidden.** RFC 9457 is HTTP's alone, so adopting it on HTTP
increases divergence across the four transports — the property #310 spent a pass improving. That
divergence is correct: a transport's wire shape is decided by the specification that owns it, not by
what the other three do. Optimising for internal consistency is what produced three shapes with no
standing outside the project, gRPC being the exception and the one transport the #310 pass found
clean. `ulo::Error` stays one trait; only the per-transport renderer differs, so the divergence is
at the wire and not in the model.

**Take F87 and F22 in the same change.** `render_error` is `pub(crate)` on every transport, which is
why a handler catching its own error type cannot produce the canonical envelope. Rewriting the
renderer without making it reachable would leave that open against a shape nobody outside the crate
can produce.

Rejected: a feature flag or config toggle emitting the legacy shape. Nothing is published, there is
no one to migrate, and a toggle is two shapes to test for as long as it exists.

**D7 — is a bindless lifecycle in scope? — DECIDED 2026-09-20: yes, as a phase after §1.** §1 gives
the function — `HttpRequest` through the pipeline to an `HttpResponse` — which does not exist today
at any granularity above one route. D7 removes the remaining assumption, that serving means owning a
socket. It is four small type questions rather than a redesign: `HttpLifecycleHandle::new` requires
a `SocketAddr`; `BoundAdapters.http` means "bound where"; `on_application_bootstrap` fires at bind;
and the graceful drain assumes ulo owns the process, where a platform adapter is called rather than
serving.

**One design point in §1 decides how much of this D7 has to carry: when the route table is built.**
If the table is built independent of any adapter, `dispatch` is reachable with no adapter at all and
portless in-process testing falls out of §1 alone, leaving D7 to cover real serverless. If the table
is still built when an adapter registers, a null adapter is needed for both. Build it independent of
the adapter — it also removes the ordering footgun that the `set_global_prefix` "Option B" was
invented to fix.

**D8 — what a global prefix is called — DECIDED 2026-09-20: `use_global_http_prefix`.** A bare
`set_global_prefix` on the factory reads as applying
to every transport, where a prefix is a URL concept and HTTP's alone. The convention that answers it
already exists: `use_global_{http,rpc,ws,grpc}_{guards,interceptors}`, and the `_http_` infix is
this project's existing statement that a global belongs to one transport. One wrinkle: `use_global_middleware` is HTTP-only by ADR-0007 and carries
no infix, so one shipped name already tells the same small lie; renaming it is a separate call and
free while nothing is published. Nest's `exclude` stays deferred on the trigger already recorded —
the first fixed-path route under a prefixed app.

Under a framework-owned table the prefix applies once, at table construction, where a collision it
creates is detectable.

---

## 8. Serverless is this gap seen from the other side

The earlier session's conclusion stands and now has a cause. Against Nest, the one genuine absence
was *programmatic synthetic-request dispatch through the full pipeline without a socket* — recorded
then as a separate feature and parked. It is F105 from the other end: there is no function taking an
`HttpRequest` and returning an `HttpResponse` through the pipeline, because the pipeline is entered
through five native routers. The per-route half of that function exists and is `pub(crate)`
(`http/mount.rs:18-26`); what is missing in front of it is the match.

`ServeContext::dispatch` is that function. With it, a Lambda, Workers or Vercel adapter is an
`into_lifecycle` that never opens a socket: map the platform's event to an `HttpRequest`, call
dispatch, map back. It is also how a test reaches the pipeline without binding a port, which is what
Nest's `getHttpServer()` plus supertest does and why every HTTP suite here boots a real server on
`127.0.0.1:0`.

ADR-0007 named this seam when it rejected the router: binding the socket is *"the natural seam for
portless dispatch (in-process testing, serverless) if that lands later."* What it did not have is
the matcher that makes the seam reachable.

Two further consequences of the table, filed rather than argued here: F120, the route table existing
only as a `tracing::debug!` line with nothing able to list it — Express's inspectable stack and
Nest's bootstrap route listing both come from holding the table; and F118, a middleware that is
pre-routing *and* path-scoped, which is not expressible today because scope resolves at mount
against the matched route. F119 is the adjacent footgun on the surface F118 lives on.

Two halves remain after §1, and neither is free. `bind()` assumes sockets across all four
transports, so a bindless lifecycle is its own change. And `create` builds the whole container
eagerly, so cold start pays DI construction — that is the parked lazy-modules item, which already
carries the caveat that a compiled binary has far less to defer than Node does.

---

## 9. Out of scope

- **Conditional requests and caching (RFC 9111).** Nothing emits `ETag` or `Last-Modified`, nothing
  reads `If-None-Match`. Whether a framework should is a scope decision, not a defect; not filed.
- **gRPC's half of F105.** tonic owns method routing, and re-implementing it means re-implementing
  tonic. The narrow remedy is a fallback service on tonic's `Routes` so an unknown method raises
  `Unrouted` rather than tonic's `Unimplemented`. **Unverified** that tonic 0.14 exposes one.
- **HTTP/2 and HTTP/3**, compression, static files, range requests and trailers — untested here,
  and mostly the adapter's or absent by design.
- **WHATWG Fetch beyond `Vary` and the `Access-Control-Allow-Methods` question.** Private-network
  access and opaque responses are unexamined. Worth one line: preflight advertises methods from
  static config while the route table knows the answer, so a preflight can approve a method the
  actual request then 405s. That is one more thing §1 makes free.
- **The remaining "not yet audited" HTTP items from `STANDARDS_AUDIT.md`**: RFC 7578 beyond the
  missing constraints, and RFC 6265 beyond the absence of any cookie surface.

---

## 10. Build sequence

**The gate is spent. The pipeline audit ran 2026-09-20 against master `654c7183`;
`PIPELINE_AUDIT.md` §9 carries the outcome and `FRAMEWORK_GAPS.md` F121–F132 the findings.**
`Dispatched` needs no third variant. Two things change, both inside PR 3:

- **One clause of ADR-0052 is wrong.** Its consequences say a miss "reaches the error chain, runs
  the global interceptors and is claimable by `#[catch(Unrouted)]`". The middle of those is not what
  either transport that already raises `Unrouted` does, and cannot be: an interceptor wraps a call
  and a miss is not one. RPC's miss runs the **global error handlers only**
  (`application.rs:1043-1063`), with no guards and no interceptors, because there is no leaf for a
  chain to wrap. HTTP's matches RPC's. Strike "runs the global interceptors" and name the tier of
  handlers a miss is offered to.
- **One constraint is added.** `dispatch` absorbs the global middleware chain, whose failure today
  reaches no error handler and answers a hardcoded 500 — probed, including an `HttpError` naming a
  status, which the same value from a module middleware honours (F125, and F81 is its
  panic-shaped half). `ServeContext` holds the chain and not the transport's globals, which is the
  cause. Writing `dispatch` is the moment that costs one paragraph; afterwards it is a second pass
  over the same function.

Everything else in this sequence stands as written. The wire audits for WebSocket, RPC and GraphQL
touch nothing decided here and need not precede any of it.

Three ADRs carry the decisions: **0052** the route table, **0053** problem details, **0054** the
response's types. Each is `proposed` until the PR that implements it accepts it, and each needs its
supersession pointer added to the ADR it revisits at that moment — 0052 into 0009 and 0007, 0053
into nothing, 0054 into nothing. Pointers are not added while an ADR is `proposed`.

| # | PR | Closes | Verification it needs |
| --- | --- | --- | --- |
| 1 | `AdapterCapabilities`, absorbing `streams_responses`, `register_ws_route`'s `Err` and rocket's listener refusal | F103, F104 | the refusal fires at `use_http_adapter` for each declared capability, per adapter |
| 2 | The core route table and matcher (`matchit`), built independent of any adapter; duplicate detection over it | F4 | a duplicate route fails `bind` with `StartupError` rather than panicking through it |
| 3 | `ServeContext::dispatch` and `Dispatched`, carrying 404, 405, `Allow`, `HEAD`, `OPTIONS`, `//` collapsing and the D2 decoding rule | F105, F107, part of F106 | `Unrouted` reaches the chain on HTTP; the §2 table's routing rows answer identically without an adapter in the loop |
| 4-8 | Migrate the adapters, one PR each: delete the matcher, the fallback, the envelope copy, the per-route registration | F106, F111 | each adapter's own suite plus the four HTTP conformance suites; the upgrade path is the spike's shape |
| 9 | Remove `register_route`, `register_ws_route` and `RequestHandler` from the SPI | — | nothing outside `crates/ulo/src` names them |
| 10 | ADR-0054: `StatusCode` and `HeaderMap` | F108, F113, F114 | two `Set-Cookie` values survive on all five; an invalid header is refused where it is written |
| 11 | ADR-0053: problem details, one renderer, `render_error` reachable | F65, F69, F87, F22 | the envelope is `application/problem+json` on all five, and a `#[catch]` handler can produce it |
| 12 | `Vary: Origin` unconditionally; a cookie surface | F109, F115 | a response to an origin-less request carries `Vary`; two cookies arrive |
| 13 | One body ceiling at 2 MiB with a knob, and multipart constraints from it | F112, F116, F27 | 413 at the same threshold on all five, carrying ulo's envelope |
| 14 | `use_global_http_prefix` | — | the prefix applies once, and a collision it creates is refused |

Left out of the sequence deliberately: F110 (a 204 carrying a body) wants D-level agreement on
which statuses admit one before it is written, and F117 (repeated query keys) is a dependency swap
that touches `Path` as well as `Query` and belongs with whichever pass takes extraction.
