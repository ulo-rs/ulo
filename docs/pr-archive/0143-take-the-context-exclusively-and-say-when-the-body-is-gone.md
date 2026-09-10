# #143 — Take the context exclusively, and say when the body is gone

Merged 2026-08-15 into `master` from `feat/handler-context-uniformity`, commit [`f875ea8`](https://github.com/ulo-rs/ulo/commit/f875ea81294cb975ac7e3b0c69fb9f9fd4dd0b82).

Two changes with one motive: the handler surface should mean the same thing on every transport, and it should not answer a question wrong when it can decline to answer.

**RPC handlers receive `&mut RpcContext`.** A shared reference works only where a context happens to be `Sync`, which tracks whether that transport streams rather than whether the handler may mutate — an implementation detail showing through the public grammar. HTTP cannot offer `&HttpContext` at all, since the wrapper's future must be `Send` and `HttpContext` is deliberately not `Sync`. `&mut` needs only `Send`, so it fits everywhere and matches the enhancer traits, leaving one reference convention in the framework instead of two.

This is not the break it looks like: `&mut` coerces at the call site, so handlers declaring `&RpcContext` keep compiling. The two dozen in the test suite were untouched and still pass; the change makes the exclusive form available and canonical rather than forcing it.

The shared reference was not buying a guarantee in any case. RPC does the same overwrite HTTP does — `ExecutionResult::Ok(data) => context.set_response(Ok(data))` — so a handler's `set_response` is already a no-op on both, and `&` was decorating one transport with a promise the other could not keep.

**`HttpContext::take_request` returns an `Option`.** The body is single-use and may be a stream, so an enhancer that reads it — a guard verifying an HMAC over the raw bytes, say — leaves nothing behind. Returning an empty body made that indistinguishable from a request that arrived empty. The dispatcher still hands the handler an empty body in that case, but it now says so at the point it decides rather than hiding the decision inside the accessor.

Same class of problem as the body-extractor arity rule: a silent wrong answer where a loud one is available.

**Tests.** The RPC case takes `&mut RpcContext` while its neighbours in the same file still take `&RpcContext`, so the coercion is covered alongside the new form. The body case runs a guard that reads the raw request and asserts the second take reports nothing while the handler's extractor is left empty.
