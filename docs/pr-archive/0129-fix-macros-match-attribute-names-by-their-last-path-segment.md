# #129 — fix(macros): match attribute names by their last path segment

Merged 2026-07-27 into `master` from `fix/attr-path-matching`, commit [`62458e6`](https://github.com/ulo-rs/ulo/commit/62458e6e31fdfae210447863d1ddc32e3f542118).

Path-qualified attribute spellings were matched inconsistently across the macro crate: the scans that detect and strip attributes accepted `#[toni::use_guards(…)]` while the collection paths demanded a single-segment path, so the attribute was consumed without effect — a guard written path-qualified compiled cleanly and the route ran unguarded. Attribute names are now matched by the path's last segment everywhere, the `attr_is` rule the codebase already applied to verb attrs and field-level `#[inject]`.

- **Enhancer collection** returns ordered pairs instead of a name-keyed map. This also fixes stacked same-kind attributes (`#[use_guards(A)]` + `#[use_guards(B)]` on one handler), which previously overwrote each other silently; they now accumulate in declaration order.
- **Standalone enhancer macros** (`use_guards`, `use_interceptors`, `use_pipes`, `use_error_handlers`) emit a compile error when no handler macro consumed them, instead of passing through. The passthrough made misplacement invisible: written above `#[routes]`, the attribute expanded first and deleted itself. The `#[routes]` flow now strips consumed enhancer attrs, as `#[patterns]`, `#[subscriptions]`, and `#[grpc_methods]` already did.
- **Same last-segment rule** applied to the lifecycle-hook scan and strip, `#[inject]` on `#[new]` constructor parameters, the marker params (`#[body]`/`#[query]`/`#[param]`), and `Clone` detection in derive lists (`#[derive(std::clone::Clone)]` no longer collides with the macro-added derive).
- **Removed**: the deprecated `#[scope("…")]` parser (no callers outside its own unit tests) and the dead `create_en(c)hancers_token_stream` pair.
- **Docs**: controller-level enhancer examples updated from the removed inline form to the `#[routes]` form; placement documented on each enhancer macro, pinned by a `compile_fail` doctest.
- **Tests**: integration coverage for path-qualified guards/interceptors, stacked guards (with ordering), path-qualified module hooks, ctor token injection, marker params, and the qualified Clone derive.

Detection limits that remain: aliased derives (`use Clone as C`) and manual `impl Clone` blocks are invisible to token-level scanning and still surface as conflicting-implementations errors.
