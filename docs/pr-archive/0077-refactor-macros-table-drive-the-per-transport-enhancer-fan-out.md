# #77 — refactor(macros): table-drive the per-transport enhancer fan-out

Merged 2026-05-06 into `master` from `cleanup/macro-enhancer-table`, commit [`350c531`](https://github.com/ulo-rs/ulo/commit/350c53100b1f1c671cfe61df96782cde5cde47fb).

Each of the four emission sites (singleton role-push,
request/transient dyn-factory, `provider_factory!` ready,
`provider_factory!` non-caching factory) restated the same nine
per-transport configuration constants — role variant, entry path,
trait path, dyn-factory trait, factory struct prefix — verbatim. A
divergence at any one site (a renamed entry type, a wrong trait path)
would silently mismatch what the other three emit. One config table in
`shared::enhancer_emit` is the source of truth all four read from.
