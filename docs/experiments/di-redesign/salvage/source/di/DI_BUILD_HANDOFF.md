# Handoff — building what the DI audit decided

For the session that implements it. The plan is `DI_AUDIT.md`; nothing here restates it.

## Status

**Every decision is settled.** `DI_AUDIT.md` records D1–D13, each with the alternatives rejected and
why. They were put to the user on 2026-09-21 and 2026-09-22 and answered. **They are not open**, and a
recommendation kept beside a decision is there as its argument, not as a choice still to make.

**This build does not choose its own place in the queue.** D7: the user runs every outstanding audit
together, groups each with its memo and build handoff in one folder, and then runs a triage pass over
the set that decides the build order and what can be built alongside what. The sequence below is
internal ordering only.

**One coupling reaches outside.** D2's answer — a token is single-bound unless declared multi, and a
clash is refused — belongs in the pipeline build's PR 5, which replaces the globals registration
surface. Shipped without it, that PR reopens F138.

## Read first, in this order

1. `DI_AUDIT.md` — D1–D13, then §9, then §1–§8 in the order a caller loses them.
2. `FRAMEWORK_GAPS.md` **F138–F162** for what this closes, and **F31**, **F45**, **F50**, **F73**,
   **F144** for the older entries it closes or spends.
3. `DOCS_AUDIT.md` **A8**, **A9**, **A27**, **B23**, **C17**, **C18**, **C19**, **C20**.
4. `PIPELINE_BUILD_HANDOFF.md` PR 5 and PR 6, which touch the same two surfaces.

Six project-memory notes hold parked threads in this territory. Five are still parked and several
were declined on purpose: `project-injectables-by-value-vs-arc`,
`project-module-as-di-participant`, `project-lazy-loading-modules`,
`project-behavioral-traits-audit`, `project-provider-context-variant-criterion`. The sixth,
`project-multi-provider-redesign`, is **no longer parked** — D2 pulls it in, and its own sketch is
what §9.1's measurement arrives at independently. Read all six before proposing anything inside them.

## Two standing constraints

**No DI defect leaves `create` by panic.** Three do today: a dispatch target declaring a
multi-provider collection (F153), a factory depending on an execution-scoped provider (F154), and the
generated execution-scoped provider resolving outside an execution. Each is the startup-failure
policy unapplied. Every PR below that touches one converts it to a `StartupError` naming the
declaration, and no PR adds a fourth.

**Pre-release: no deprecation shims (D13).** The crate is unpublished and the first release is a
beta. Delete rather than deprecate, rename rather than re-export under both names, and keep no alias.
This applies to `provide!` and the four macros, `init = "…"`, `Injectable`, and `.global()`. The rule
is about keeping an API alive, not about keeping a diagnostic that explains a rename — and for
`init = "…"` no migration error is kept either, its pre-release audience being this repo's five call
sites and the docs site, both of which the change updates.

## The two ADRs

Written first, on their own branch, both `proposed`. Each flips to `accepted` in the PR that
implements it, which is also when its supersession pointers are added.

- **The container answers an ambiguous question, or refuses it.** D1, D2, D3 — what a duplicate token
  means, what module boundaries a runtime lookup may cross, and whether lifecycle ordering is a
  contract. One subject: today the container resolves an ambiguity by taking whatever a hash map
  hands it first. ADR-0029 decided one case of D2, the global-export clash, so this gets a pointer
  under 0029's title rather than an edit to it.
- **A provider declaration is a value.** D4, D8, D9, D10, D11, D12 — the surface, the always-async
  factory, the one constructor, the vocabulary. Carries the rule that decided D9: *where a language
  affordance is being copied rather than a design, check the affordance exists.*

## The sequence

Five groups. Order matters inside each; between groups only where a row says so.

### A — the declaration surface

| # | PR | Closes |
| --- | --- | --- |
| 1 | The value surface beside the macros: `Provide::{value, factory, alias}`, `T::provide()`, `Declaration::{under, multi}` as one blanket over `ProviderFactory`, `IntoFactory` per arity, async-only | — |
| 2 | Migrate every call site; delete `provide!` and the four macros; `__ulo_provider_factory` leaves the public surface | F161, F162, F143, F148, F50 |
| 3 | `#[module]`'s four keys parse expressions; `exports:` gains both builder forms; `Extension<T>` implements the declaration trait; one `.provider(value)` on the builder and one `.controller(value)` | F144, C18 |

PR 2 subsumes the pipeline build's PR 6, which fixes the `#[use_guards]` const case alone. Whichever
runs first, the other drops that part.

### B — construction

| # | PR | Closes | After |
| --- | --- | --- | --- |
| 4 | `#[new]` is the only constructor: `init = "…"` deleted from `#[injectable]`, `#[controller]`'s refusal message deleted with the key, `init = "from_request"` deleted | F145, F156, F157 | — |
| 5 | `#[default]` beside `#[new]` is refused naming both; the remaining construction mistakes report against the user's own items | F158, F159 | 4 |
| 6 | A lifecycle hook that cannot fire is refused rather than dropped: the `#[injectable]` path sees the impl block, the execution and transient rejections become reachable, a per-call target's init `Err` fails the call | A9, C17 | 4 |

### C — the container's answers

| # | PR | Closes | After |
| --- | --- | --- | --- |
| 7 | Multi becomes ordinary token semantics; a duplicate single binding is refused naming both; `APP_GUARD` becomes a multi token | F138, F31, the multi redesign | 2 |
| 8 | Insertion-ordered storage for the four maps, and the lifecycle ordering contract on every phase | F140 | — |
| 9 | `.or_global()`, narrowed to exported and global tokens; the two-modules-one-token ambiguity refused where startup sees it and reported as `AmbiguousModule` where it does not | F141, F142 | 7 |

### D — the resolvers and the panics

| # | PR | Closes |
| --- | --- | --- |
| 10 | The `Vec<Arc<dyn Trait>>` branch extracted and shared with the controller resolver | F153 |
| 11 | The two twin field resolvers collapsed into one parameterised on the map and the execution | F155 |
| 12 | A factory declaring an execution-scoped dependency is refused at startup | F154, A27 |

PR 10 is invisible to every other row: nothing else in this plan touches `controller_attr.rs`. It
must be scheduled explicitly or the panic survives the whole build.

### E — surface

| # | PR | Closes | After |
| --- | --- | --- | --- |
| 13 | `Injectable` becomes `Registration` | D11 | 2 |
| 14 | `UloApplication::new` becomes `pub(crate)`; `ModuleRef`'s two by-type lookups call `token_of`; the GraphQL crates' two hand-formatted tokens | F147, C19, C20 | — |
| 15 | A startup check joins the module identity | F152 | — |
| 16 | `Arc<dyn Provider>` across the SPI, the container and the macro expansions | F150 | 13 |
| 17 | The unreachable retry budget deleted; the per-module cycle reports through `find_dependency_cycle` | F146, F149 | — |
| 18 | The in-source doc defects; `ExtensionsFactory` renamed; `Module::_get_controller_by_token` deleted; the `CLAUDE.md` rows naming `ProviderContext` and `provider_scope` | F151, F160, B23 | 20 |

PR 18 loses half of F160 to PR 20, which deletes `ExtensionFactory<T>` outright rather than renaming
it. Run 20 first and PR 18 renames one name instead of disambiguating two.

### F — what a provider hands out (D14)

Added 2026-09-22, after the rest of this handoff was written. D14 is in `DI_AUDIT.md`.

| # | PR | Closes | After |
| --- | --- | --- | --- |
| 19 | An injectable is shared: `Arc<T>` for singleton and execution scope, by value for transient; the field is written `Arc<T>`; the provider macro's added `#[derive(Clone)]` is deleted with it | D14, and F7's cause | 16 |
| 20 | `ExtensionFactory<T>` deleted; the container answers any `Extension<T>` token from the execution's bag | F181, F144's `Extension` half, half of F160 | 19 |
| 21 | `Request` loses its two precomputed maps | F180's first half | — |
| 22 | `ulo::http::Request` deleted; `RequestPart` registered twice — a `Provider` under `token_of::<RequestPart>()` and `FromContext<HttpContext>` | F180 | 19, 21 |

**PR 19's derive removal is not optional and is the easy thing to drop.** Shipping the sharing and
leaving `#[derive(Clone)]` (`crates/ulo-macros/src/provider_macro/instance_injection.rs:237`) pays
the whole migration for half the benefit and keeps F7's diagnostic alive. The two belong in one PR
for that reason.

**PR 21 stands alone** and can run at any point: the two maps have one caller between them, in a
test. PR 22 is what needs 19.

**Not in this build:** F45 (three `TypeId`-keyed maps) is a design question with no cheap fix, filed
and left. F73 is the pipeline build's PR 12. F47's `Body` collision is untouched — PR 22 clears the
`Request` half of that entry and the response-body/request-extractor pair is a separate call.

## Verification each PR needs

Every probe below was written and run during the audit and removed. Each is the test its PR owes,
promoted from a `println!` to an assertion. Each ran beside a control, and the control is part of the
test.

- **PR 1** — every shape in §9.2 compiles: three constructors, a type's own declaration, both
  modifiers, and the two chained. A sync closure into `factory` is refused. A qualified `Token<T>`
  const, a bare const and a runtime `String` all serve as tokens. A typed token refuses a value of
  the wrong type, and a string token does not.
- **PR 2** — §4's spelling matrix, one case per cell, refusals as `trybuild` snapshots.
- **PR 3** — `exports: ["TOKEN"]`, `exports: [path::T]`, `exports: [Generic<T>]` and
  `controllers: [path::C]` compile, and the exported token resolves from an importing module.
- **PR 4** — a provider whose init method is named `from_request` compiles like any other.
- **PR 5** — `#[default]` beside `#[new]` fails with a message naming both. Falsify without `#[new]`,
  which must keep compiling.
- **PR 6** — an execution-scoped provider carrying a lifecycle hook fails to compile with a message
  naming the scope. Falsify against singleton scope, and against a guard.
- **PR 7** — two providers under one token fail `create` naming both; two `APP_GUARD` registrations in
  one module and across two modules both run, in declaration order. Falsify with a single
  registration, which must start and run once.
- **PR 8** — a provider injecting another fires `on_module_init` after it and its shutdown hooks
  before it; adding an unrelated provider changes neither. The audit reproduced the flip with four
  different added names; the test needs at least two.
- **PR 9** — a provider in a module that exports nothing is unreachable by `.or_global()`; two imports
  exporting one token fail `create` naming both. Falsify against an exported provider in a global
  module, which must stay reachable.
- **PR 10** — a controller and a provider declaring the identical `Vec<Arc<dyn Trait>>` field both
  resolve the same contributions. The provider side is the control and passes today.
- **PR 12** — a factory over an execution-scoped dependency fails `create` with a `StartupError`
  naming both. Falsify over a singleton, which must start.
- **PR 14** — `UloApplication::new` is uncallable from the integration-test crate.
- **PR 16** — no behavioural test; the gate is that the workspace builds with no flags and the suite
  passes.
- **PR 19** — three assertions in one test, and they are D14 made executable. Two providers injecting
  one **singleton** hold the same object (`Arc::ptr_eq`); two providers injecting one
  **execution-scoped** provider inside one request hold the same object; two providers injecting one
  **transient** provider do not. The first two fail today and are what the PR changes; the third is
  the falsification and must keep passing. Add a fourth: an injectable whose fields are not `Clone`
  compiles, which is what says the derive is gone rather than merely unused.
- **PR 20** — a provider declaring `Extension<CurrentUser>` resolves with no `providers:` entry for
  it, and a guard's write is visible to it. Falsify by removing the guard's write, which must read
  `None` rather than fail.
- **PR 21** — the one call site moves to `Query<T>`; the probe is that nothing else referenced either
  map.
- **PR 22** — `RequestPart` serves both positions in one handler: injected as a field and taken as a
  parameter beside a body extractor. The audit compiled both halves separately
  (`EXTRACTION_AUDIT.md` §2); this is the pair in one place.

Every audit probe ran beside a control before being believed, and three measured something other
than their subject until the control caught it — two wrong probes and one extractor that returned
two-line function bodies. `feedback-self-test-the-checker` is the rule and it earned its cost three
times in this pass.

## The branch

None exists. Nothing was committed by the audit and master is clean at `654c7183`.

## The divergence gate — stop and ask

Approval of this build is approval of what was read and understood. If the build has to depart from
it, it pauses and reports rather than proceeding on judgement.

Divergence: a signature that differs from what §9 or D1–D13 describes; an alternative recorded as
rejected turning out to be necessary; a finding that reorders the sequence, removes a PR or adds one;
a verification that cannot be written as above; a decision whose rejected alternatives turn out not
to have been the only ones.

Not divergence: naming inside a PR, file placement, test names, the order of two PRs inside a group.

On divergence: state what the memo said, what the tree requires, and what the options are. Wait.

## The Law

Per `feedback-the-law-review-protocol`: before every commit message and every PR title and body, a
dedicated agent reviews the text against `~/.claude/commits.md`, `~/.claude/pull-requests.md`,
`~/.claude/prose-discipline.md` and `~/.claude/posture.md`. Run it in the background and surface only
its findings. Give it the branch and commit, an instruction to read the diff in full, a numbered list
of the technical claims to verify hardest, and how many prior passes found factual errors.

A finding from the Law is a claim, not a verdict.

## Per-PR checklist

Walk it. The recorded failure mode is a checklist written and not walked.

- **Production text this change falsifies** — doc comments, the ADRs, and the workspace `CLAUDE.md`
  in the untracked parent, where a repo-wide grep misses it. Its *Declaration forms*, *Modules &
  DI*, *Execution-model DI* and *ulo-macros* rows are touched by PRs 1–9 and 13, and the
  *Execution-model DI* row already carries A9's marker, which PR 6 clears.
- **One check in its plainest form, with no flags.** `--all-features` hides `cfg` bugs and clippy
  cannot pass workspace-wide (F80, duplicated as F38).
- **Merge with `--merge`, never `--squash`.**

## Bookkeeping

New findings go to `FRAMEWORK_GAPS.md` as `F<n>`, continuing from the highest there (F162 at the time
of writing). Do not move `Last synced HEAD` or the `Docs status` section in `CLAUDE.md`. Append one
line per merged change to `DOCS_AUDIT.md` §18 naming the pages it invalidates; PRs 1–9, 13 and 18
each invalidate a docs page on providers, modules, tokens, lifecycle hooks or enhancers, and PR 2
invalidates every page that shows `provide!` or one of the four provider macros.
