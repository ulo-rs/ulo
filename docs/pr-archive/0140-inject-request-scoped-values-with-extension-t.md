# #140 — Inject request-scoped values with Extension<T>

Merged 2026-08-14 into `master` from `feat/extension-di-view`, commit [`03642ab`](https://github.com/ulo-rs/ulo/commit/03642abda1063fd7c3696dc298466cc2829eae41).

A value a guard attaches to the request now reaches any provider that declares it, however far below the controller it sits.

The bag alone could not do that. It reaches enhancers, which hold the context, and handlers, which get it as a parameter or off the client — but a service two constructions down has neither, so anything the guard computed had to be threaded through every call signature between them. That threading is the coupling the bag exists to remove.

**`Extension<T>`** is a typed view of one payload in the request's bag, resolved like any other dependency. It holds the bag rather than a copy of the value, so a guard and a service constructed before it address the same slot. `set` / `get` / `with` / `with_mut` / `take` mirror the bag's own surface, with `Clone` required only on `get`.

**The bag injects too.** `Extensions` is registered globally, so code reading several payload types can take it directly instead of declaring a view per type. No registration needed — there is only one of it.

**Registration** is one `Extension::<T>` entry in a module's provider list, alongside anything else. Each `T` is a distinct DI token, so the container cannot produce them from a single registration; teaching it open generics would remove the line and is its own piece of work.

**Scope.** Request-scoped, so the container refuses it inside a singleton — one request's values would otherwise be served to every later request. That confines it to HTTP, the same as request scope generally: WebSocket gateways and RPC controllers are constructed once at startup, and their handlers read the bag off the client or the context, which they already hold.

**Tests.** The integration coverage puts the reader two constructions below the controller, with no route and no context of its own. A second module pins per-request isolation by writing on the guard's first run only — a value still present on the next request could only have survived from the previous one. Unit tests cover the view's surface, including a payload that is not `Clone`, and that two views of different types over one bag do not collide.

**Example.** `request_scoped_context` shows the whole shape: a guard authenticates and attaches the caller, and both the controller and an audit service read it without it appearing in a signature between them.
