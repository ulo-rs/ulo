# #174 — A validated extractor reads only what it wraps

Merged 2026-08-23 into `master` from `fix/validated-reads-only-what-it-wraps`, commit [`09e7bcc`](https://github.com/ulo-rs/ulo/commit/09e7bccdc41669726ca93a16950dd40cfe5c8597).

`Validated<E>` extracts through the extractor it wraps, so `Validated<Query<T>>` and `Validated<Path<T>>` read request metadata and leave the body for a body extractor on the same handler.

The wrapper's `FromContext` impl called `extract_body`, which takes the request off the context before the inner extractor runs, whatever that extractor needs. A parts-only inner extractor had the body consumed on its behalf, and anything reading it afterwards found `AlreadyRead`. The handler macro classified the wrapper as a body consumer for the same reason, so `Validated<Query<T>>` beside `Json<U>` was rejected at compile time — naming the query parameter as one of two body readers.

- **Extractors** — `Validated<E>` implements `FromContext<HttpContext>` by delegating to `E::extract`. Its `FromRequest` impl goes, and the bounds narrow from `E: FromRequest + ValidatableExtractor + Send` plus a `Display + Send + Sync + 'static` clause on `E::Error` to `E: FromContext<HttpContext> + ValidatableExtractor`. `from_context.rs` is left holding only extractors that do read the body.
- **Macro** — `ExtractorKind::Validated` carries the kind of what it wraps and `takes_the_body()` recurses into it, the shape `Optional` already had. Detecting either wrapper shares a `first_type_argument` helper.
- **Docs** — `Validated`'s doc comment described a body wrapper, and destructured its example as `Json(dto):` where the pattern is `Validated(Json(dto)):`.
- **Tests** — `Validated<Query<T>>` and `Json<U>` on one handler, asserting the body arrives and the query is still validated. Falsified by reverting the three source files: the handler does not compile, with the wrong parameter named.

Breaking: `Validated<E>` no longer implements `FromRequest`. Anything reaching it through that trait takes `FromContext<HttpContext>` instead.

390 integration tests pass.
