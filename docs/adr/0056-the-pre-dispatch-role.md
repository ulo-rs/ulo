# 0056 — One role per position: the pre-dispatch role reaches every transport

Status: proposed

Supersedes the middleware half of [ADR-0007](0007-pre-routing-global-chain.md): `Middleware`
narrows to the one anchor where its signature is right, before dispatch, and reaches RPC, WebSocket
and gRPC; the module-scoped anchor is retired, and its contents become interceptors.

## Context

On HTTP one role exists twice with two contracts. `Middleware` and `Interceptor<HttpContext, _>`
both wrap a call and answer it:

| | `Middleware` | `Interceptor` |
| --- | --- | --- |
| transports | HTTP alone | four |
| anchor | pre-routing (global) or post-routing on the matched route (module) | controller or method |
| sees | the whole `HttpRequest`, and may replace it | `&HttpContext` |
| its failure reaches the chain | no, stringified, or never | typed |
| per-execution instance | no | yes |
| scoped by path | the module form, against the registered pattern | no |

A middleware's failure has three fates, and the road is chosen by what the error happens to be: an
`HttpError` from a module middleware renders directly and the chain does not run; a domain error from
one reaches the chain as `MiddlewareFailure` carrying its `Display` text; anything at all from a
global middleware answers a hardcoded `500`, because `ServeContext` holds the chain and not the
globals. The same domain error raised one layer down, in the handler, arrives typed and is claimed by
`#[catch(MyDomainError)]`. The loss is the middleware path's, not the error's.

The cause is position. Module middleware runs around the whole target pipeline, outside the error
chain, in the same position as the global role and merely scoped per route:

```
global middleware → routing → module middleware → error chain → guards → interceptors → handler
```

Its failures do not reach `#[catch]` because of where it sits, and fixing them in place would mean
giving one layer a claimant that the layer above it does not have.

The global form earns its place: it runs where no route has been chosen, which nothing else can do,
and it is what observes a `404` and a same-port upgrade. The module-scoped form is a controller-level
interceptor with a worse error contract. What it has over one is request replacement, which matters
less after routing because the context already holds the request, and path scoping — which is only
in that surface because the pre-routing surface has no route table to scope against.
ADR-0052 gives it one: the framework owns the HTTP route table.

An execution-scoped `Middleware` registers no role, and `create` refuses it with a message naming two
conditions that both hold and not the scope that is the cause.

## Decision

**One role per position, each named for the position it holds.**

```
outer error chain          ← globals only; claims a pre-dispatch failure, and the miss
 └─ pre-dispatch role      ← four transports, path-scoped, tower mounts here
     └─ dispatch           ← target selection
         └─ inner error chain    ← global + target + method handlers
             └─ guards
                 └─ interceptors
                     └─ handler
```

| nesting | why |
| --- | --- |
| outer chain above pre-dispatch | a pre-dispatch role can fail, so something must claim it |
| pre-dispatch above dispatch | it must see misses, which is the whole reason the role exists |
| inner chain below dispatch | its target and method tiers need a target; a miss has none, so a miss gets the globals |
| chain above guards | a guard's refusal must be claimable |
| guards above interceptors | a guard admits, an interceptor wraps an admitted call |
| interceptors innermost | they wrap the call |

Two chain positions, one seam: the inner one always resolves to an answer, so the outer one only ever
meets a pre-dispatch failure or a miss, and no error is offered twice.

**A pre-dispatch role on all four transports** — request in, answer out. `Middleware` narrowed to the
one anchor where its signature is right, given the typed-error contract, and reaching RPC, WebSocket
and gRPC. "Before the target is known" is a well-defined moment on every transport: RPC matches a
pattern and WebSocket matches an event, both inside the framework. On gRPC the role anchors above
tonic's router, in the `Server::builder().layer(…)` stack, which covers services registered through
`add_service` as well.

**Interceptors for everything post-dispatch.** `configure_middleware`'s anchor goes, and its contents
become `#[use_interceptors]`.

**Path scoping moves to the pre-dispatch registration**, over the framework's route table, the only
place that knows the routes.

**Tower mounts at the pre-dispatch anchor.** A tower `Service` is `http::Request → http::Response`,
which is that anchor's signature and not the interceptor's; post-dispatch there is no request left to
hand it. With path scoping that gives "this layer on `/admin/*`".

**The pre-dispatch role's failure reaches the global error handlers**, on every transport, in place of
the literal `500` the HTTP serve context writes.

**A non-singleton `Middleware` is refused by a diagnostic naming the scope.**

## Consequences

- **One behavioural change.** Code moved from a module middleware to an interceptor moves *inside* the
  chain: an `Err(HttpError)` that rendered directly now reaches `#[catch]`. Work that belongs outside
  the chain goes to a path-scoped pre-dispatch role instead; nothing is lost, it moves.
- `MiddlewareConsumer`, `configure_middleware`, `for_route`, `for_routes`, `exclude_route` and
  `apply_tower` on the module go. `use_global_middleware` becomes the one registration, with a path.
- A global middleware's failure is claimable by a `#[catch]` handler on every transport, and a panic
  in one is delivered as `PanicRecovered` rather than a literal `500`.
- RPC, WebSocket and gRPC gain a role that sees a call before its target is chosen.
- The name is kept. Express means pre-routing by "middleware", so narrowing the role makes the
  borrowed word accurate.

## Roads not taken

**Keeping the module anchor with a fixed error contract.** It preserves two ways to write one thing.

**Leaving it.** It keeps the path axis and the pre-routing axis in different registration surfaces
because one role is wearing two, and keeps a module middleware's failure outside the chain.

**Path scoping inside dispatch.** Path scoping is built on the framework-owned route table rather
than as a third `Dispatched` variant.
