# #173 — An execution need not have a transport, and resolution takes one

Merged 2026-08-23 into `master` from `feat/resolve-in-an-execution`, commit [`84e80b9`](https://github.com/ulo-rs/ulo/commit/84e80b94ebe59dfe65ebc739a1fd1a3bb3e605c6).

Resolving a provider by hand takes the execution it belongs to, and an execution no longer has to be a transport's.

`resolve` took HTTP request parts and built an execution out of them. Naming a transport was the only way to obtain one, so a CLI tool, a job or a test that wanted a request-scoped provider wrote `http::Request::builder().body(()).unwrap()` — a request whose method, URI and headers nothing reads. `ModuleRef` had no resolution at all: `get` builds outside any execution, and a request-scoped provider handed none panics from inside its generated factory.

- **Core** — `StandaloneContext` and `ProviderContext::Standalone`, an execution with no transport behind it, answering `HandlerContext` from the same shared struct the four transport contexts delegate to. `ProviderContext::standalone()` builds one; `ProviderContext::None` still means no execution at all, which is what a request-scoped provider is refused by. The enum becomes `#[non_exhaustive]`.
- **Resolution** — one verb taking the execution: `resolve` and `resolve_by_token` on the application context, on `ToniApplication`, and on `ModuleRef`. Two resolutions against one execution share a request-scoped instance; two executions build two; which is happening is readable at the call site. `get` refuses a request-scoped provider by name instead of panicking from a macro expansion.
- **Internals** — the six resolution methods lose their per-method copies of the module scan and the downcast, and the two by-module ones stop holding the container borrow across the await.
- **Docs** — ADR-0022 records what an execution is made of and why none of it is wire state. The scope docs, the init-scan comments and the constructor bridge described request scope as needing an HTTP request.
- **Tests** — the refusal, resolution in a standalone execution, in an HTTP one and in an RPC one, and the sharing that puts the cache on the execution rather than on the caller: one execution twice is one instance whether the module or the application asks. Each refusal test was falsified against the behaviour it replaces. The validation tests stop fabricating an HTTP request none of them read.

Breaking: `resolve` and `resolve_by_token` change signature on `ToniApplicationContext` and `ToniApplication`. Callers pass `HttpContext::from_parts(parts).into()` where the execution is genuinely an HTTP request, and `ProviderContext::standalone()` where it never was. Adding an enum variant breaks exhaustive matches on `ProviderContext`; `#[non_exhaustive]` means the next variant does not.

Not covered: a transient provider whose own fields are request-scoped still panics when built outside an execution. The refusal reads the provider's declared scope, and that one is `Transient`; catching it needs a walk of its dependencies.
