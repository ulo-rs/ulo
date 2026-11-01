# Handoff — the DI and core-fabric audit

**SPENT.** The pass ran 2026-09-21 against master `654c7183`. Its memo is `DI_AUDIT.md`, its findings
are `FRAMEWORK_GAPS.md` F138–F152, and its build is `DI_BUILD_HANDOFF.md`. It cleared F37, added a
cross-reference marking F38 and F80 as one finding filed twice, and left seven decisions open for the
user in `DI_AUDIT.md`. Kept for the brief it set; do not run it again.

---

The brief as it was issued follows, unedited.

Run this as its own session. It is the foundation pass: the user's framing is a full re-walk of the
DI system and the core fabric — *"its structure, cleanliness, dx, redundancy, polishness, betterness
and improvement."*

## Scope

The container, the instance loader, the dependency graph, the scanner, module metadata and identity,
`ModuleRef`, scopes and the execution cache, the token language, the provider SPI, `DynamicModule`,
multi-providers, lifecycle hooks, and the `Injectable` value model. One subject, not per transport —
`di/` is transport-blind by rule, and the four files that are not are already F73.

Out of scope: dispatch and the enhancer pipeline, which the pipeline audit took; the wire, which has
its own handoffs per transport; extraction, which has its own.

## Why it goes first among the remaining four

Everything sits on it. The pipeline audit's Phase 1 PR 5 rewrites the globals registration surface
and PR 11 changes how a non-singleton provider registers its roles — both are DI edits made from
outside DI, which is a sign the boundary is not where it should be. Take this pass before anything
else re-enters that code.

**Do not start it mid-way through the pipeline build.** Phase 2 changes trait signatures the DI
macros emit. Either finish Phase 1 and 2 first, or accept that this pass will read code about to
move.

## What is different about this one

Category (3) in full, under the three-category test in `project-the-spec-conformance-seam`: no
specification mentions dependency injection, modules, scopes or tokens. Nothing here is a conformance
question and nothing crosses a wire. "Better" means the design is coherent, each concept earns its
place, and one idea is not spelled three ways.

**The token language is the thread most likely to pay.** The user's own words on it: *"I feel token
inconsistency and the way token and the overall language of the DI."* Three spellings reach the
container — a type (`token_of::<T>()`), a string, and a `Token<T>` const — and they are not
interchangeable. F50 is one instance of the seam, and it was found from the enhancer side rather than
from DI's own.

## Already filed — read before probing, do not re-derive

`FRAMEWORK_GAPS.md`: **F73** (four files under `di/` serve one transport, `MiddlewareConsumer` the
substantial one), **F50** (a `Token<T>` const is not nameable where it is documented), **F45** (three
TypeId-keyed maps written three times), **F44** (nineteen public items reachable through no path),
**F43** (three names resolving to two things depending on the path), **F37** (module instantiation
retries on a substring of an error message), **F6** (string-anchored identity collides across two
versions of one crate), **F31**, **F16**.

`DOCS_AUDIT.md` §17/§20/§22: **A9** (lifecycle hooks on an execution- or transient-scoped provider
compile and are never called, and the rejection is unreachable), **A27** (`provider_factory!(…,
scope = "request")` does not give one construction per execution), **C17** (a per-call dispatch target
discards its startup hook's `Err` and never fires its shutdown hooks), **C18** (`controllers:` and
`exports:` accept only a bare `Ident` where `providers:` accepts an expression), **C19** (two by-type
lookups bypass `token_of`, against ADR-0028), **C20** (the GraphQL crates hand-format two tokens).

**Closed by the pipeline build, so read its outcome rather than the entry**: F51, F126, F133, F134,
F135 — the role-registration split by scope, and the globals matrix.

Parked in project memory, each with a reason and none of them re-derivable from the code:
`project-injectables-by-value-vs-arc` (injectables clone out by value, the root of the `Clone`
requirement), `project-multi-provider-redesign`, `project-module-as-di-participant` (the user leans
against), `project-lazy-loading-modules` (unbuilt, the user has active intent),
`project-behavioral-traits-audit`, `project-provider-context-variant-criterion` (an owed doc edit).
Read all six before proposing anything in their territory; several were declined on purpose.

Prior art: `project-module-identity` for ADR-0029, `project-token-format-unification` for ADR-0028,
`project-circular-dependency-model`, `project-codebase-architecture`.

## Method that worked on the two passes before this

- **Probe, then falsify.** Every probe disabled and re-run to confirm it measured what it claimed. A
  clean first run proves nothing — `feedback-self-test-the-checker`.
- **The spelling audit applies here directly.** `feedback-the-spelling-audit`, both halves: which
  spellings a macro refuses, *and* which spellings it accepts and treats differently. The second half
  was found by the pipeline audit in the enhancer attribute; the token language is the other place
  one intent has several spellings.
- **Measure before collapsing.** `AUDIT3_DISPATCH.md` §4's rule — differing lines after substituting
  the noun — is what decides whether two similar things are one thing. It has been right twice.
- **Confidence marking per claim** — Probed / Read / Measured / Fetched / Unverified — declared near
  the top of the memo.

## Deliverable

1. A memo, `DI_AUDIT.md`, at this level beside the others. Form: `feedback-the-audit-memo-form`.
2. Findings to `FRAMEWORK_GAPS.md`, numbered from the highest there.
3. Its own build handoff, the way `PIPELINE_BUILD_HANDOFF.md` was written.
4. A decisions section the user answers, with the alternatives rejected recorded beside each.

Nothing is committed by an audit.
