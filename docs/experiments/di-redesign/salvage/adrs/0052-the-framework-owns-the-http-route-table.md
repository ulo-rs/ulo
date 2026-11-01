# 0052 — The framework owns the HTTP route table

Status: proposed

Replaces per-route registration on `HttpAdapter` with one dispatch entry the adapter calls per
request, so route matching, the miss, and everything decided from the route table happen once in
core instead of five times in the adapters. Supersedes the registration half of ADR-0009 and
answers the alternative ADR-0007 rejected.

## Context

`HttpAdapter::register_route` hands the adapter one `Arc<dyn RequestHandler>` per route, and
`RequestHandler::handle` answers an `HttpResponse` and nothing else. The route table therefore
lives in the adapter's native router and the framework holds no copy, so no value means "nothing
matched".

The other transports divide this differently. RPC hands the adapter a pattern list and one
callback; WebSocket hands it a path and one callback per gateway. Both keep the matching decision
in the framework, and both raise `Unrouted` for a call nothing claims. HTTP and gRPC gave the
decision away and raise nothing. The correlation is not incidental — reporting the miss requires
having made the decision.

What the delegation costs, measured against master `654c7183` over all five adapters:

- **Three adapters match paths themselves anyway.** Rocket performs all of its dispatch that way
  behind per-method catch-all mounts; salvo and actix keep a second route table to answer 405,
  because — in ADR-0007's own account — "salvo's router cannot distinguish a method mismatch from
  no match". So the native router is carrying the work on two of the five.
- **The five disagree on the wire.** `GET //probe` against a route at `/probe` answers 404 on axum
  and poem, 405 on actix, and 200 on salvo and rocket. `HEAD` on a GET-only route answers 200 on
  axum and poem and 405 on actix, salvo and rocket, against RFC 9110 §9.1: "All general-purpose
  servers MUST support the methods GET and HEAD." A 405 carries no `Allow` header on poem, against
  RFC 9110 §15.5.6's MUST. `Path<String>` percent-decodes three different ways.
- **The error envelope is written by hand six times** — once in core and once in each adapter.
- **Nothing can list the routes.** There is no framework-side table to publish, so there is no
  route listing, no duplicate-route check before the native router panics (F4), and no way to print
  the pipeline resolved for a route.

The divergence tracks who decides. Where the answer comes from the underlying server — absolute-form
request lines, `Expect: 100-continue`, chunked bodies, duplicate `Content-Length` — the five agree.

**Three ADRs have already moved a piece of routing into core for this reason.** ADR-0011 took
trailing-slash canonicalisation: "The fix cannot live in the adapters. Five routers have five
strictness models." ADR-0012 took the parameter syntax, counting "five adapters times two syntaxes
… ten translation points to keep in sync". ADR-0007 took the middleware anchor. This decision takes
what those three were working around.

## Decision

`HttpAdapter` carries two methods:

```rust
pub trait HttpAdapter: Send + Sync + 'static {
    fn capabilities(&self) -> AdapterCapabilities { AdapterCapabilities::default() }
    async fn into_lifecycle(self: Box<Self>, target: BindTarget, ctx: ServeContext)
        -> AdapterResult<HttpLifecycleHandle>;
}
```

`register_route`, `register_ws_route` and `streams_responses` are removed, and `RequestHandler`
leaves the public SPI with them. `ServeContext` carries the entry point:

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

An adapter's work per request is to build the `HttpRequest`, call `dispatch`, and either write the
response or perform its native upgrade with the callbacks it was handed. Path canonicalisation, the
global chain, matching, 404, 405, `Allow`, `HEAD`, `OPTIONS`, percent-decoding and the error
envelope move inside `dispatch` and are written once.

**The route table is built independent of any adapter.** Routes mount inside `use_http_adapter`
today, which is what makes the table an adapter-shaped thing and what the parked `set_global_prefix`
had to work around by moving resolution into `bind()`. Building it as its own step makes `dispatch`
reachable with no adapter registered at all, so an in-process test and a portless handler both
follow from this decision rather than from the lifecycle change after it.

**Capabilities are declared as data, not as trait methods.** `AdapterCapabilities` is read once at
`use_http_adapter`, carrying at least `streams_responses`, `same_port_upgrade`, `adopts_listener`
and `max_request_body`. Three refusal mechanisms collapse into it: a defaulted trait method
(`streams_responses`), an `Err` from a registration call (`register_ws_route`), and an `Err` at bind
(rocket refusing a pre-bound listener, ADR-0013). Every mismatch is then refused in one place,
before a socket opens, and a new capability is added without a trait change.

**The matcher is `matchit`.** It is a data structure with no runtime dependency, already in the
lock file under axum, and its parameter syntax is `{param}` — the canonical spelling since
ADR-0012, which is why the axum adapter mounts paths unchanged.

**`ServeContext` keeps its name.** ADR-0047 gives the `*Context` family a meaning — a handle on one
execution — and `ServeContext` is deliberately not one of those; it spans a serving session. Only
its method is renamed, `execute` to `dispatch`.

## Consequences

- `Unrouted` is raised on HTTP, so a miss reaches the error chain, runs the global interceptors and
  is claimable by `#[catch(Unrouted)]` like the RPC and WebSocket misses already are.
- One envelope, one `Allow` computation, one percent-decoding rule, one matcher. `HEAD` is answered
  from the matching GET route and `OPTIONS` from the `Allow` value, both once rather than five times
  or not at all.
- A duplicate route is a check over one table before bind, in place of an axum panic through it.
- `app.routes()` becomes a read, and with it the pipeline resolved for a route.
- A global prefix applies once, at table construction, where a collision it creates is detectable.
- An adapter that never opens a socket becomes expressible — a serverless or in-process handler maps
  a platform event to an `HttpRequest`, calls `dispatch`, and maps back. ADR-0007 named this seam
  when it rejected the router. Because the table is built independent of the adapter, dispatching
  without one needs no further change; what remains for real serverless is the lifecycle, where
  `HttpLifecycleHandle::new` requires a `SocketAddr`, `BoundAdapters` reports where each transport
  bound, `on_application_bootstrap` fires at bind, and the graceful drain assumes ulo owns the
  process. That is its own decision, taken and scoped as a following phase.
- Five adapter crates are rewritten at their routing seam. Rocket's shrinks the most; salvo's and
  actix's lose a fallback handler and a matcher; axum's and poem's lose per-route registration and
  gain one fallback service.
- The SPI breaks for any out-of-tree adapter. Nothing is published to crates.io yet, so the break
  costs nothing now and would be a 1.0 break later.
- One extra match per request on axum and poem, where the native router was doing it. Unmeasured.
- What stays the adapter's: accepting connections, parsing the message, body framing,
  `Expect: 100-continue`, TLS, HTTP/2, the upgrade handshake, and writing the response. Refusals
  below that line are not recoverable through this SPI — rocket answers its own HTML 400 to an
  unrecognised method inside its parser, before any ulo code runs.

## Verified before deciding

The design turned on whether a same-port WebSocket upgrade can happen at the dispatch point rather
than on a mounted route. Poem and salvo bind the upgrade to an extractor on the matched route and
were the likeliest to fail.

Both were patched on a throwaway branch at `654c7183`: no WS route mounted natively, the upgrade
decided by a function seeing only the ulo `HttpRequest`, and the handshake performed from the native
request shell retained across that call. `ws_same_port_upgrade_and_echo` passes on both. Disabling
the decision makes the same test fail with `404 Cannot GET /ws`, which is what says the passing run
was the new path and not the old one.

It works because both adapters already retain the native request across the global chain — poem's
comment says the shell "keeps state http parts cannot carry — the WebSocket upgrade slot", and
salvo's `mem::take` moves the upgrade state with it — and both already round-trip the 101 response
through ulo's `HttpResponse`, which keeps status, headers and body alone. The upgrade is driven by
the request slot and the handshake response is ordinary data, so `Dispatched::Upgrade` needs no
out-of-band channel. Rocket is the third witness: having no usable native routing, it already
decides the upgrade from the ulo request and signals it through a magic response header, which is
this variant hand-rolled.

## Roads not taken

**Report the miss and keep the seam.** Give `RequestHandler` a way to answer "no match" and leave
matching in the adapters. It closes the miss and removes no hand-written matcher, so the five keep
disagreeing about `//probe`, `HEAD` and `%2F`. Rejected: the report was never the whole defect.

**Leave it, per ADR-0007.** That ADR rejected "a ulo-owned router (`matchit`) in the serve path" on
the grounds that it would "re-implement path matching the hosts already do well", and that "Nest
never does this". The first is falsified by measurement that did not exist in July 2026: the hosts
match well individually and do not agree, and two of the five are not doing the work at all. The
second is the argument the project has already ruled against — an ecosystem's popular framework is
not a standard, and Nest delegates to two Node servers with compatible semantics where ulo delegates
to five Rust servers with five. ADR-0007 also kept the door open in the same paragraph, naming
`matchit` as legitimate "for a future adapter-independent portless handler".

**One adapter instead of five.** Considered and rejected for now. Four of the five sit on hyper and
converge after this change, so the five crates are two protocol stacks — hyper and actix-http —
wrapped five ways. What keeps them separate is interop: a tower layer, an actix middleware crate, a
rocket fairing, or an application already built on one of those servers. The five are also the
conformance asset that produced the evidence above; one implementation would have hidden all of it.
A first-party `ulo-http-hyper` as the reference implementation is the follow-on worth taking, and it
is additive rather than a cull.
