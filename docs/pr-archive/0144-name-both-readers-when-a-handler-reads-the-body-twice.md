# #144 — Name both readers when a handler reads the body twice

Merged 2026-08-15 into `master` from `feat/body-extractor-arity-check`, commit [`5b2ae7a`](https://github.com/ulo-rs/ulo/commit/5b2ae7a3480d10e2ed4ebeb9085cc1e5ec40b516).

A handler declaring two body-consuming extractors now gets an error that names them.

It was already rejected — the generated code moves the body into the first reader, so the second is a use-of-moved-value. But the diagnosis was the problem:

```
error[E0382]: use of moved value
  --> src/orders.rs:14:1
   |
14 | #[routes]
   | ^^^^^^^^^ value used here after move
   |
   = move occurs because value has type `http::request::Parts`
help: borrow this binding in the pattern to avoid moving the value
   |
14 | ref #[routes]
```

It points at the attribute rather than the handler, blames a type the user never wrote, and suggests syntax that does not exist. Now:

```
error: `dto` and `raw` both read the request body, and it can only be read once.
       Take the body with one of them and derive the rest from it, or take `HttpRequest` and read it yourself.
  --> src/orders.rs:17:36
   |
17 |     fn create(&self, dto: Json<Dto>, raw: Bytes) -> Body {
   |                                           ^^^^^
```

**Why the body cannot be shared.** It is single-use because it may be a stream — `RequestBoxBody` is an `UnsyncBoxBody`, and there is nothing to hand a second reader. This is the same fact behind `take_request` returning an `Option`.

**`Unknown` types are excluded from the count.** They are generated on the body-consuming path but handed an empty body, which is what lets several custom parts-only extractors sit on one handler. Only the named readers compete for the body, so that arrangement keeps working.

`Option<Json<T>>` counts — the check recurses through the wrapper — and so does taking `HttpRequest` directly, since that hands over the body along with everything else.

**Tests.** Unit tests over the signature parser rather than a compile-fail harness, since the repo has none: one body extractor beside parts extractors passes, two are rejected with both names in the message, an optional wrapper still counts, the raw request counts, and several custom extractors coexist.
