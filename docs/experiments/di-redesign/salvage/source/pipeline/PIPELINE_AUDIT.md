# Pipeline audit — the order, the seam, and where a failure is shaped

Opened 2026-09-20. Scope set by `PIPELINE_AUDIT_HANDOFF.md` and by the user, who asked for
*"betterness, polish, dx, best practice, clean, maintenance and solid"* on the grounds that
*"none of these had existed together, they came in one after the other"* — an audit and a
foundation to build on rather than a conformance pass.

Subject: middleware, guards, interceptors, error handlers and the chain, handlers, panic recovery,
the execution order, and how enhancer tiers resolve and stack, across all four transports as one
thing. Middleware folds in as HTTP's half (ADR-0007).

**Where the findings live.** `FRAMEWORK_GAPS.md`, F121–F132, plus a correction appended to F60.
This memo is the plan and does not restate them.

**Not a build.** Nothing is committed and no branch exists. Four probe files were added under
`integration-tests/tests/integration/` and removed; `git status` is clean at `654c7183`. Work this
recommends branches per section.

## Confidence marking

- **Probed** — run this session against master `654c7183` through the real macro and DI path, not a
  hand-built model of it. Each probe carries its own falsification: the same pipeline with the one
  thing under test removed, run in the same process. Where a falsification is named below it was
  run, not reasoned.
- **Read** — read off the source, cited by file and line in the ledger entry.
- **Measured** — a count or a normalised diff, with the normalisation stated.
- **Fetched** — another project's published documentation, retrieved and quoted rather than recalled.
  One claim written from memory did not survive it and is corrected in §5.
- **Unverified** — believed, not established. Marked inline, and not acted on before falsification.

Nothing here rests on a specification. Under the three-category test in
`project-the-spec-conformance-seam` the pipeline is category (3) — no published standard mentions a
guard, an interceptor or an error chain — so "better" means coherent, not conformant. The one
category-(1) question the handoff asks was checked: **nothing the pipeline puts on the wire is
specified anywhere except the HTTP error body, which ADR-0053 already holds.** A guard rejection, a
panic and a claimed error each render through the transport's existing envelope; RPC's and
WebSocket's envelopes are the framework's own invention with no standard over them, and gRPC's is
`tonic::Status`, whose shape is the gRPC spec's and which ulo does not write. §8 records the one
place the framework's *own* vocabulary disagrees with itself, which is an internal-consistency
question, not a conformance one.

## The shape of it

Two independent problems and a field of consequences. **The first is that the pipeline has one
order and three ways of deciding it** — §1, §3 and §7 are that, and they can be taken in any order.
**The second is that a failure has one seam and four ways of reaching it** — §2, §4 and §5. §6 is
the synthesis of both on HTTP, where the same role exists twice with two contracts, and §5 and §6
together settle the lifecycle the build works to. §9 is the gate the HTTP build waited on and is the
shortest section. §10 records the decisions, all taken 2026-09-21; everything above it is the
evidence for them, and `PIPELINE_BUILD_HANDOFF.md` is the sequence that implements them.

---

## 1. The order is not the order that was written

**(Probed)** `#[use_guards(AuthGuard{}, RoleGuard)]` runs `RoleGuard` first. The cause is a split
that happens in the macro and is never undone: `enhancer_vecs` sorts one declaration into a token
vector and an instance vector, and `extend_with` pushes every token entry before every instance
entry. The interleaving is gone before any resolver sees it.

The tiers go with it. On HTTP the controller-level and method-level attributes are merged into one
map *before* the split, so a controller-level inline guard runs after a method-level token guard —
and the same two guards written in token form run controller-first, which is what says the form is
the cause rather than the tier. For error handlers the declaration order stops mattering at all:
with one of each form on one impl, the inline handler claims in either written order and the token
handler is consulted in neither.

**`#[use_guards]`'s own documentation states both rules this breaks** — the hierarchical tier order
and "within each level, guards execute in the order specified". It is the only place a caller learns
either. F121.

This is the finding with a caller-visible security shape: an authentication guard written first and
a role guard written second run in the written order, or do not, depending on whether one of them
took a constructor argument.

## 2. A middleware's failure has three fates, and one of them is the chain

**(Probed, three ways with three falsifications.)** ADR-0035 made the error chain the one error
seam and #302–#312 hoisted every transport onto it. A middleware's failure reaches it by one of
three roads, and the road is chosen by what the error happens to be:

| what a middleware returns | what reaches the chain | what the caller gets |
| --- | --- | --- |
| `Err(HttpError::custom(429, …))`, module-scoped | nothing — the chain does not run | 429, the middleware's own envelope |
| `Err(MyDomainError)`, module-scoped | `MiddlewareFailure`, carrying the `Display` text | whatever a handler catching `MiddlewareFailure` answers |
| anything at all, registered globally | nothing — `ServeContext` holds no handlers | a hardcoded 500 |
| the same `MyDomainError` from the route handler | `MyDomainError`, typed | `#[catch(MyDomainError)]` claims it |

The fourth row is the falsification: the same error, the same registered handler, raised one layer
down, arrives typed. So the loss is the middleware path's, not the error's.

Each row has a local reason and none of them survives being put beside the others. The `HttpError`
downcast says the author "constructed the wire shape themselves" — so does a handler returning
`HttpError`, and that one is offered to the chain. The stringify discards a
`Box<dyn Error + Send + Sync>`, which is the exact type `ChainError` borrows. The global row has no
handlers to reach because `ServeContext` was given the chain and not the globals, and it is the row
where a caller most wants one: an authentication middleware registered globally cannot answer 401.

F123, F124, F125. F81 is the panic-shaped instance of the third row and one fix closes both.

## 3. A refusal has already paid for everything it refused

**(Probed.)** `guards_for` and `interceptors_for` build every entry in the slice and then return;
the caller evaluates the guards afterwards. On HTTP, RPC and WebSocket both calls happen before the
first guard is asked anything. A request refused by the guard at index 0 logs
`built:guard0, built:guard1, built:interceptor, ran:guard0`.

On the `Factory` arm, building an entry is the execution-scoped provider's own DI resolution, with
its dependencies. So the work a refusal exists to avoid is done before the refusal is heard.

**gRPC is the one that orders them** — guards, then interceptors, at `grpc/runtime.rs:43-51` — which
makes this three-against-one rather than a uniform choice. It still builds every guard before
evaluating the first. F122.

## 4. Scope stops at eight of the twelve enhancer roles

**(Probed, with both falsifications.)** A provider's roles are registered two ways. The singleton
path probes for all twelve — middleware, eight guard and interceptor kinds, four error-handler
kinds. The non-singleton path loops over the eight and nothing else.

So `#[injectable(scope = "execution")]` on an `ErrorHandler` or a `Middleware` registers no role,
and `create` refuses with a message naming two conditions that both hold:

```
HTTP ErrorHandler 'ScopedHandler' not found in registry. … make sure the provider is in
the module's `providers` list.
```

The trait is implemented. The provider is in the list. The scope is the cause and is not named. The
same types at singleton scope resolve and an execution-scoped *guard* resolves, which is what places
the boundary. F126.

This also spends a clause of F51, which filed the error-handler half as an asymmetry nothing
refuses. Something does refuse it, with the wrong diagnosis, which is worse than the gap F51
described.

## 5. The chain answers outside the interceptors — and should

**(Probed.)** An interceptor that stamps a header on every response it returns puts it on a 200 and
not on the 418 an error handler recovered the call with. The chain runs above the whole interceptor
stack on all four transports, so an interceptor sees the `Err` on the way out and never sees what
answered it.

**(Fetched.)** Nest divides it the same way. Its documented lifecycle runs *"8. Interceptors
(post-request: route → controller → global) · 9. Exception filters (route → controller → global) ·
10. Server response"*, and *"as soon as an uncaught exception is encountered, the rest of the
lifecycle is ignored and the request skips straight to the filter."* Its `catchError` operator is
the opt-in for an interceptor that wants the error, which is what `match next.run(ctx).await` already
is here. A memo draft claimed this from memory as a caveat; fetched, it is the same design.

**Decided: the chain stays outside, and the argument is not that it works today.** An interceptor can
fail, deliberately since #309. So the chain has to be above the interceptors or an interceptor's
failure has no claimant — which is ADR-0035's own one-seam rule, and it forces the nesting rather
than merely permitting it.

The corollary answers what felt wrong: **an inner layer never observes what its outer layer does.**
The interceptor is inside the chain, so of course it does not see the chain's answer. That is what
nesting means. The expectation that it should came from the word rather than from the design, and
stamping every answer is by definition an *outermost* job — a layer can only stamp answers produced
inside it.

**So the gap closes at the other end.** The pre-dispatch role (§6) wraps dispatch and the chain both
and already sees chain-produced responses: `CorsMiddleware` depends on exactly that, reading
`next.run().await?` and pushing headers onto whatever came back
(`crates/ulo/src/http/middleware/cors.rs:172-190`). F127 therefore closes as a documentation fix on
`Interceptor` plus D2 reaching all four transports, not as a change to where the chain runs.

**The lifecycle this settles on**, each nesting justified rather than inherited:

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
| outer chain above pre-dispatch | a pre-dispatch role can fail, so something must claim it — this is F125 |
| pre-dispatch above dispatch | it must see misses, which is the whole reason the role exists |
| inner chain below dispatch | its target and method tiers need a target; a miss has none, so a miss gets the globals — RPC's behaviour today |
| chain above guards | a guard's refusal must be claimable |
| guards above interceptors | a guard admits, an interceptor wraps an admitted call; an interceptor must not run its before-phase for a call that is refused |
| interceptors innermost | they wrap the call |

Two chain positions, one seam: the inner one always resolves to an answer, so the outer one only ever
meets a pre-dispatch failure or a miss, and no error is offered twice. Nothing anywhere fails without
a claimant, which is the property position A exists to protect and which is complete for the first
time in this shape.

What each role sees, which is the same fact read the other way:

| role | wraps | sees on the way out |
| --- | --- | --- |
| outer chain | everything | a pre-dispatch failure, or a miss |
| **pre-dispatch** | dispatch and the target pipeline | **every answer**, the inner chain's included |
| **inner chain** | guards, interceptors, handler | **every failure**, from any segment below |
| guards | interceptors and handler | — it admits or refuses |
| interceptors | the handler | the typed answer, or the typed error |

## 6. On HTTP, one role exists twice with two contracts

Synthesis of §2 and §4 rather than a new finding. `Middleware` and `Interceptor<HttpContext, _>`
both wrap a call and answer it:

| | `Middleware` | `Interceptor<HttpContext, _>` |
| --- | --- | --- |
| transports | HTTP alone (ADR-0007) | four |
| anchor | pre-routing (global) **or** post-routing on the matched route (module) | controller or method |
| sees | the whole `HttpRequest`, and may replace it | `&HttpContext` |
| answers | `Result<HttpResponse, Box<dyn Error>>` | `Result<HttpResponse, HttpError>` |
| its failure reaches the chain | no / stringified / never (§2) | typed |
| per-execution instance | no (§4) | yes |
| scoped by path | the module form, against the *registered pattern* (F119) | no |
| continuation | `NextHandle::run(self)` | `InterceptorNext::run(Box<Self>)` |

**The global form is a distinct role and earns its place**: it runs where no route has been chosen,
which nothing else can do, and it is what observes a 404 and a same-port upgrade. **The
module-scoped form is a controller-level interceptor with a worse error contract.** What it has over
one is request replacement, which matters less after routing because the context already holds the
request, and path scoping — and path scoping is only in that surface because the pre-routing surface
has no route table to scope against. ADR-0052 gives it one.

That is also why F118 and F119 read as a split axis: the path axis and the pre-routing axis sit in
different registration surfaces because one role is wearing two. Retiring the module anchor closes
F73 as well, `MiddlewareConsumer` being the substantial file under `di/` that still names a
transport.

**Where the module anchor actually sits, which is the thing that explains §2.** (Read.)
`handle_request` runs the module chain *around* `execute_controller_logic`
(`crates/ulo/src/http/pipeline.rs:99-114`), and the error chain runs inside that call
(`:189-206`). So the nesting today is:

```
global middleware → routing → module middleware → error chain → guards → interceptors → handler
```

Module middleware is **outside the error chain**, in the same position as the global role and merely
scoped per route. Its failures do not reach `#[catch]` because of where it sits, not because someone
forgot to offer them — F123 and F124 are consequences of the position, and that is why fixing them
in place would mean giving one layer a claimant that the layer above it does not have.

**The design this settles on.** One role per position, each named for the position it holds:

- **A pre-dispatch role on all four transports** — request in, answer out. `Middleware` narrowed to
  the one anchor where its signature is right, given the typed-error contract, and reaching RPC,
  WebSocket and gRPC for the first time. Nothing forced it to be HTTP's alone: RPC matches a pattern
  and WebSocket matches an event, both inside the framework, so "before the target is known" is a
  well-defined moment on every transport. It was HTTP's alone because middleware is an Express idea
  and HTTP was the only transport when it arrived.
- **Interceptors for everything post-dispatch.** `configure_middleware`'s anchor goes, and its
  contents become `#[use_interceptors]`.
- **Path scoping moves to the pre-dispatch registration**, over ADR-0052's table, which is the only
  place that knows the routes. F118 and F119 close there.
- **Tower mounts at the pre-dispatch anchor.** A tower `Service` is `http::Request → http::Response`,
  which is that anchor's signature and not the interceptor's; post-dispatch there is no request left
  to hand it. Path scoping then gives "this layer on `/admin/*`", which is what per-route tower means
  in practice — and is more than `apply_tower` offers today, since its `for_route` scoping does not
  match concrete paths at all (F119).

**Keeping the name is defensible.** Express means pre-routing by "middleware", so narrowing the role
makes the borrowed word accurate rather than less so.

**One behavioural change the ADR must state.** Moving module-middleware code to an interceptor moves
it *inside* the chain: today a module middleware's `Err(HttpError)` renders directly, and as an
interceptor the same `Err` reaches `#[catch]`. That is the improvement, and it is a change for anyone
who relied on the old path. Work that genuinely belongs outside the chain goes to a path-scoped
pre-dispatch role instead, so nothing is lost — it moves.

## 7. What the #300–#312 collapse left behind

**(Read and measured.)** The series set out to make one pipeline out of four and got most of the
way. What remains is measurable against the line `AUDIT3_DISPATCH.md` §4 drew — collapse below
about six differing lines after substituting the transport noun, split above thirty-five.

- **Three resolvers, 5–8 differing lines.** Normalising the transport noun and the key name across
  `dispatch/resolve/{rpc,ws,grpc}.rs` leaves 5 lines between RPC and WebSocket and 8 between RPC and
  gRPC, of which three are gRPC carrying no metadata and two are a field name. §3 of that memo
  scoped `Resolved<T>` to replace them and the two nine-argument constructors and
  `ResolvedGrpcEnhancers`; it was not built. F128.
- **gRPC's second interceptor chain, 86 lines against the shared 42.** F60 counted four; #305
  collapsed three. The remaining one exists because its leaf is a closure, and AUDIT3 §2 probed the
  constructor that fixes exactly that — `closure_leaf::<T, _>`, which does not exist in the tree.
  F129, and F60's counts are corrected in place.
- **`safe_render` three times.** The fallback literals below them stay per transport; the recovery
  around them does not differ.
- **F57 is the live instance of F128**: three transports merge their enhancer tiers on every call
  because the resolvers store the tiers apart. One `Resolved<T>` that stores the merged list per key
  closes it at no cost.
- **F74** remains: eight per-transport aliases the closing checklist expected to go.

None of this is caller-visible. What it costs is that a change to how a tier resolves is three edits
and nothing checks them against each other — which is how F57 happened.

## 8. The vocabulary disagrees with itself in three places

**(Read, one probed.)**

- **A guard's `false` is `Forbidden` on a message and `Unauthorized` on a WebSocket connect** — the
  same guard, the same connection, one frame apart. The connect path raises `WsError::AuthFailed`
  instead of `GuardRejection`, so `#[catch(GuardRejection)]` has nothing to match and the wire
  `kind` changes. gRPC hand-builds both a refusal and a guard panic rather than converting, so the
  message for a guard panic differs from the message for the same panic caught one level out. F130.
- **Three public items have no producer**: `GuardRejection::with_reason`, `PanicRecovered::during`,
  `PipelineSegment::Other`. The first is the visible one — `GuardRejection.reason` is always `None`
  because `Guard::can_activate` answers `bool` and has nowhere to put a reason, so the field
  documents an input the trait cannot supply. F131.
- **Four pieces of production text the tree falsifies**, including `Interceptor`'s per-transport
  list of `R`, which still gives HTTP a bare `HttpResponse`, and the not-found diagnostic announcing
  "A errorhandler". F132.

**And one cause under the first of those.** Each transport keeps a second error vocabulary beside
`ErrorKind` — eight named variants on `HttpError`, seven on `WsError`, three on `RpcError` — each
mapping back onto a kind at render time. Picking a variant therefore picks a kind, and F130 is what
that costs: `WsError::AuthFailed` read as the right variant for a refused connection and brought
`Unauthorized` with it, where the same guard refusing a message renders `Forbidden`. Nobody chose
that. F136, settled in §10.

## 9. The gate — what this changes in ADR-0052

**The HTTP build is released.** One clause of 0052 changes, one constraint is added to it, and
`Dispatched` needs no third variant.

**The clause.** 0052's consequences say a miss "reaches the error chain, runs the global
interceptors and is claimable by `#[catch(Unrouted)]`". The middle of those three is not what the
two transports that already raise `Unrouted` do, and cannot be: an interceptor wraps a call and a
miss is not one. RPC's miss runs the **global error handlers only** (`application.rs:1043-1063`),
with no guards and no interceptors, because there is no leaf for a chain to wrap. WebSocket's
reaches its gateway-level and global handlers through the ordinary failure path. HTTP's should match
RPC's. Strike "runs the global interceptors" and say which tier of handlers a miss is offered to.

**The constraint.** 0052 moves the global middleware chain inside `dispatch`. That is the moment
F125 is cheap: the chain's own failure must reach the same error handlers the miss does, rather than
the literal 500 `ServeContext::execute` writes today. Written into 0052 as a requirement on
`dispatch`, it costs one paragraph; written later it is a second pass over the same function.

**Not a change.** No third `Dispatched` variant is wanted. F118's path-scoped middleware is built
*on top of* the framework-owned table rather than inside 0052, and §6 says whether it is built at
all is a design call first.

---

## 10. Decisions — settled 2026-09-21

All five were put to the user and answered. Each records what was chosen and what was rejected, so
the build reads a decision rather than a recommendation. The rejected alternatives are kept because
they are the part nothing else in the tree will carry.

**D1 — a declaration's order is the contract. SETTLED.** The entries a declaration names run in the
order written, whatever spelling each uses. One ordered vector whose element is a token or a value,
replacing the token vector beside the instance vector. *Rejected:* documenting the split instead —
it leaves a caller needing to know which spelling they used in order to know which guard runs first,
and the ordering is security-shaped. Closes F121, and makes `#[use_guards]`'s own documented rule
true for the first time.

**D2 — `Middleware` narrows to a pre-dispatch role on four transports; the module anchor is retired.
SETTLED.** §6 carries the design, the tower answer and the one behavioural change. *Rejected:* keeping
the module anchor with a fixed error contract, which preserves two ways to write one thing; and
leaving it, which keeps F118, F119, F123, F124 and F126's middleware half as a set. Closes F73, F118,
F119, F123, F124 and the middleware half of F126.

**D3 — the error chain stays outside the interceptors. SETTLED, position A.** §5 carries the argument:
an interceptor can fail, so the chain must be above it or that failure has no claimant, and an inner
layer not observing its outer layer is what nesting means. *Rejected:* moving the chain inside the
stack, which trades the one-seam property for the stamping case; running it at both ends with the
inner one always resolving, which would hand every interceptor an answer and take away the typed
error that logging wants; and widening `Interceptor` so its after-phase carries both, which costs a
signature change to buy what the pre-dispatch role already provides. F127 closes as a documentation
fix plus D2.

**D4 — a guard answers `Result<(), E>`. SETTLED.** `Ok(())` admits, `Err(e)` refuses with a typed
reason, `E: Into<T::Error>`, so `Err(MyAuthError::TokenExpired)` reaches `#[catch(MyAuthError)]`
exactly as a handler's error does, and the framework still attaches the guard's index. *Rejected:*
`bool` as it stands, which is why `GuardRejection.reason` is documented and never set; and
`Result<bool, E>`, which keeps two refusal paths that render differently — the class F130 already is.
Closes F131, and is the Rust spelling of what Nest expresses by throwing. Breaks a trait every
application implements, so it rides the same record as D2.

**D5 — two ADRs. SETTLED.** One on *the pipeline's order*, carrying D1, D3, D4, the §8 vocabulary and
the API pass below; one on *the pre-dispatch role*, carrying D2 and superseding the middleware half of
ADR-0007. Neither is written yet: an ADR records a decision taken, and these were taken today.

### The API pass, settled with them

Raised in the same conversation and answered. Filed as F137, and it rides the pipeline ADR.

- **One type parameter on all three roles.** `Guard<Http>`, `Interceptor<Http>`, `ErrorHandler<Http>`,
  `Next<Http>`. `Transport` becomes **public and sealed** — public so it can be named, sealed because
  a user's own marker would get no adapter and no dispatch. *Rejected:* an `Answer` associated type on
  `ExecutionContext`, which was the route that avoided the break. It fails on its own terms: an answer
  is a property of a transport, not of an execution, and `StandaloneContext` implements
  `ExecutionContext` while having no answer at all — Route 1 would force it to invent one. Nothing is
  lost in expressiveness: a cross-transport guard is `impl<T: Transport> Guard<T>`. Breaks every
  enhancer impl in every application, mechanically.
- **A concrete `Next<T>`**, with `InterceptorNext` made private. The middleware surface already has
  this shape — `NextHandle` is concrete and its trait is private, and its own doc says *"no public
  `Next` trait."* The interceptor never got it.
- **Context first everywhere**, so `ErrorHandler::catch(&self, ctx, error)`; **`handle_error` becomes
  `catch`**, aligning the trait with `#[catch]`; **one result alias**, not `MiddlewareResult` beside
  `HttpHandlerResult`.
- **A capability trait for the answer-touching case** — `HasHeaders` implemented for `HttpResponse`
  and `GrpcReply` and nothing else, so `impl<T: Transport, R: HasHeaders> Interceptor<T>` covers the
  two transports where stamping means something and is refused at compile time on the two whose wire
  has no header slot. *Rejected:* Nest's mechanism, verified as runtime narrowing through
  `getType()` / `switchToHttp()` — the only option TypeScript leaves, and one whose `else` branch
  returns a wrong answer with nothing to report it, where a bound would refuse to compile.

**The rule under both rejections, stated so the next capability meets it:** *a trait carries what is
true of everything that implements it; anything narrower goes on a narrower trait that only the things
which have it implement.* It is why `Answer` does not go on `ExecutionContext` and why a header slot
does not go on `Interceptor`.

### The enhancer grammar, settled

Three spellings, three lifecycles, each distinguishable by grammar alone:

| written | is | lifecycle | DI |
| --- | --- | --- | --- |
| a bare path — `AuthGuard` | a **type name** | whatever its scope says | yes |
| a value expression — `RateLimiter::new(100)`, `X{}` | a **value** | built once at startup, shared | no |
| a closure — `\|ctx\| …`, `\|_\| …` | a **constructor** | built per execution, at this site | through `ctx` |

Additive: nothing existing breaks, and F56 closes because per-execution construction becomes reachable
from a declaration site. The `{}` is not decoration — it disambiguates a unit-struct value from a type
name, which the grammar requires — so all three forms are told apart with no convention to remember.
*Rejected:* deleting the value form, which would lose the shared-stateful case a rate limiter needs.

### Also settled

- **The globals matrix.** One registration surface per transport per role, accepting a token or a
  value, for all four roles including error handlers. Today two of sixteen cells resolve from DI.
  Closes F133, F134, F135, and removes the question of which of two spellings injects.
- **`Token<T>` consts in `#[use_guards]`** — recognise a const resolving to a `Token<T>`, and stop
  documenting `APP_GUARD` as the example, since naming a global at a controller is confused on its
  own terms. Closes F50.
- **One error vocabulary.** `ErrorKind` is it; each transport's error becomes
  `Status { kind, message }` plus `Custom { status, message }` plus `AppError`, with today's
  constructors kept as sugar. Closes F136, and F130 with it — the second vocabulary is what made a
  WebSocket connect guard's refusal `Unauthorized` where the same guard refusing a message is
  `Forbidden`.

## 11. Out of scope, and why

Each of the first three now has a handoff of its own, written 2026-09-21 alongside this memo.

**Extraction** — `EXTRACTION_AUDIT_HANDOFF.md`. The pipeline handoff asked for a decision at the
start: taken here only where extraction touches the pipeline, which is F84 — a failure answers the
caller from inside the generated wrapper, above the guards and outside the chain. That is §2's first
row in another place, and §2 was decided in favour of the chain, so F84 goes to that pass with the
precedent rather than the answer. The extractors themselves are a surface of their own.

**The renderer's reachability** — `RENDER_AUDIT_HANDOFF.md`. F22 and F87 are ADR-0053's, per the
handoff. That handoff's first job is deciding whether anything is left once 0053 and this build have
landed, since F65, F69, F129 and F136 are all claimed by work already decided.

**DI and the core fabric** — `DI_AUDIT_HANDOFF.md`. Not deferred by this pass so much as exposed by
it: F126 and F133–F135 are DI defects found from the enhancer side, and the pipeline build edits DI
from outside twice. The user named it as the next pass.

**The wire.** HTTP's is done; WebSocket's, RPC's and GraphQL's have their own handoffs. §8's
vocabulary finding is about which event the framework raises, not about how it is framed.

**Performance** — `PERFORMANCE_AUDIT_HANDOFF.md`, and it runs last. F122 and F57 are both
allocation-shaped and neither is measured; no finding in this repo is filed on cost alone, and that
pass has to build something to measure with before it can change that.
