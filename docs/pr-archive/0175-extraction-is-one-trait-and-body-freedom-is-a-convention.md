# #175 — Extraction is one trait, and body-freedom is a convention

Merged 2026-08-23 into `master` from `refactor/extraction-is-one-trait`, commit [`440eae3`](https://github.com/ulo-rs/ulo/commit/440eae3d8d40342308d66bca249e1348a5071432).

Every HTTP extractor implements `FromContext<HttpContext>` directly. `FromRequestParts` and `FromRequest` are removed, so HTTP has the one shape WebSocket and RPC already had.

Neither shorthand bought a capability. `ctx.request()` borrows the request parts without touching the body, so an extractor written against `FromContext` is as body-free as one written against `FromRequestParts` — and the restriction that trait imposed reaches its author and nothing else, since the controller macro classifies parameters by type name and a custom extractor is `Unknown` whichever trait it carries. `FromRequest` cost a second impl on every body extractor, whose whole body was `extract_body::<Self>(ctx).await`.

- **Extraction** — metadata extractors borrow `ctx.request()`; body extractors call `take_body::<Self>(ctx)`, which yields the request once and hands the second asker a `BodyAlreadyRead` carrying its name. `?` lifts that into `BodyExtractionError`, so a body extractor is one impl. No blanket impls remain in the extraction path.
- **`Request`** — gains an inherent infallible `from_parts`. Its provider and factory build one while holding parts and no context, which the trait had forced through an `Infallible` `Result` and an `.expect` that could not fire.
- **Macro** — the `#[query]` marker codegen goes through `FromContext` like everything else. Classification by type name is unchanged; it decides which parameter reads the body, not how any of them is read.
- **Docs** — ADR-0023 records the decision and what it gives up. ADR-0015's "two shorthands" section is marked superseded; the rest of it stands.
- **Example** — `custom_extractors` writes its nine extractors against the context, including the one that composes three others.

What this gives up is precise: an extractor author can now call `ctx.take_request()` while meaning to read a header, and the handler's body extractor then fails on a live request rather than the mistake being unwritable. That failure names the extractor and logs at error level, and the alternative bought a guarantee no part of the framework reads.

Breaking for every custom extractor. A metadata one changes its signature and reads `ctx.request()`; a body one folds its two impls into one. `toni::FromRequestParts` and `toni::FromRequest` are replaced at the crate root by `toni::FromContext` and `toni::take_body`.

390 integration tests and 86 lib tests pass. The diff is 312 lines added against 312 removed.
