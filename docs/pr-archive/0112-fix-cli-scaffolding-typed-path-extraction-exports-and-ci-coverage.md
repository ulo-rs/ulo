# #112 — fix: CLI scaffolding, typed path extraction, exports, and CI coverage

Merged 2026-07-13 into `master` from `fix/master-audit-fixes`, commit [`38a4cf5`](https://github.com/ulo-rs/ulo/commit/38a4cf5e6e1324945f623379a0daf5a58e1b1c96).

Repairs a set of breakages on master that a rustfmt-only CI could not see: `toni new` produced a project that did not compile, four feature-gated health tests still used the removed inline-struct controller form, `Path<T>` failed at runtime for every non-string target, and the custom-extractor validation pattern was unimplementable downstream. CI is expanded so each of these classes fails loudly from now on.

**CLI**
- Templates scaffold against the current API: split `#[controller]`/`#[routes]` form, `ToniFactory::new().create_with(...)` + `use_http_adapter` + `start()`, and the toni-axum dependency. `generate resource` registers controllers under the struct name rather than the pre-collapse `*ControllerFactory`, and its module-import insertion anchors on the new template text.
- `--version` derives from the crate version; it was hardcoded to 0.0.1 while the crate is at 0.1.2.

**Extractors**
- `Path<T>` deserializes structs through serde_urlencoded — the same deserializer `Query` uses — so numeric and bool targets parse from raw path segments; bare scalars keep a JSON fallback with the string form tried first. 8714a7f addressed the scalar-shape half of this but left every non-string target failing on string→number coercion. Covered by unit tests and an end-to-end `Path<i32>` test.
- `ValidatableExtractor` and `ValidationError` are exported; both were unnameable outside the crate despite `Validated<E>` requiring the trait bound.

**Macros**
- Generated extraction-error bodies resolve serde_json through a public `toni::serde_json` re-export instead of the consumer's crate graph, so downstream apps no longer need their own serde_json dependency. The scaffold drops it accordingly.

**Tests**
- The four DB-crate health tests (sqlx, seaorm, redis, mongodb) migrate off the inline-struct controller form removed in ADR-0004; they sit behind the `integration` feature, which CI never compiled.

**CI**
- Three new jobs: `cargo check --workspace --all-features --all-targets`, the hermetic test suites (toni lib + integration-tests), and a scaffold smoke test that compiles `toni new` + `generate resource` output against the checkout via `[patch.crates-io]`.
