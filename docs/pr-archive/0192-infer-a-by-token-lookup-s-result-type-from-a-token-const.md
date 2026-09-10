# #192 — Infer a by-token lookup's result type from a Token const

Merged 2026-08-27 into `master` from `feat/typed-token-lookup`, commit [`e9e33d1`](https://github.com/ulo-rs/ulo/commit/e9e33d11c7cbf43e937716ae1fb37396562deb69).

`IntoToken` gains a type parameter: the type the key claims to resolve to. A string claims any type, so `get_by_token::<T>("NAME")` chooses `T` at the call site exactly as before. A `Token<T>` const claims only its own parameter, so the by-token lookup APIs (`get_by_token`, `resolve_by_token`, `get_from_by_token`, and their `ModuleRef` counterparts) infer the result type from the const — no turbofish — and binding the result to any other type is a compile error rather than a runtime downcast failure.

`let _: u32 = app.get_by_token(NAMED_VALUE)` with `NAMED_VALUE: Token<String>` compiles on master and fails at runtime; here it is rejected at compile time. The `token_format` integration test carries the inference form.

Breaking only for external code that names the trait bare in a bound (`impl IntoToken` now means `IntoToken<()>`); string and `String` keys implement every claim, so such bounds keep accepting strings.
