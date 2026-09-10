# #92 — docs(adr): record provider/enhancer/lifecycle DX decisions

Merged 2026-06-09 into `master` from `docs/adr-provider-dx`, commit [`0ca1ae4`](https://github.com/ulo-rs/ulo/commit/0ca1ae416bfb6f919d30a28fe6f3bbb713452ad8).

Establishes `docs/adr/` (the repo had no decision-record convention) and records the three architectural decisions behind #91 whose rationale isn't visible from the code.

## Records
- **0001 — Dispatch, don't detect.** The autoref bridge that lets a struct macro work without seeing the impl, and the silent UFCS-through-`Arc` trap a contributor will otherwise re-hit.
- **0002 — One `#[injectable]` form; marker-free roles.** Why a provider is a plain struct + trait impls, why it's an attribute (not a derive), and why there's no `#[guard]`.
- **0003 — Lifecycle: bridge only where the impl is invisible.** Providers use the bridge; controllers/modules keep a direct scan. The guardrail against "unify everything onto the bridge."

Docs only — no code changes. Format is lightweight MADR, append-only.
