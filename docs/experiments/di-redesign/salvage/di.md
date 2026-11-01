# Salvage: dependency injection

**Subject.** The DI and core-fabric audit of 2026-09-21/22 against master `654c7183`, its build
handoff, the DI build rows of `BUILD_SEQUENCE.md` (rows 31-54, Halts 3-9, the decisions and the
pause), and the three documents written on 2026-10-01 when the build paused: the core model, the
review of an outside visibility proposal, and the outside chat's proposals as the user pasted them.
The audit's findings are `FRAMEWORK_GAPS.md` F138-F162; the build's own reviews filed F254-F285;
the core model and the review filed F286-F292.

**Read in full** (`docs/experiments/di-redesign/salvage/source/di/`, word counts by `wc -w`):

| File | Words |
| --- | --- |
| `DI_AUDIT.md` | 5464 |
| `DI_AUDIT_HANDOFF.md` | 874 |
| `DI_BUILD_HANDOFF.md` | 2551 |
| `DI_CORE_MODEL.md` | 5754 |
| `MODULE_VISIBILITY_REVIEW.md` | 2506 |
| `OUTSIDE_CHAT_PROPOSALS.md` | 3932 |
| total | 21081 |

Also read: `source/BUILD_SEQUENCE.md` lines 140-175, 218-256 and 544-1070 (the DI rows and halts);
`FRAMEWORK_GAPS.md` entries F6, F7, F31, F37, F44, F45, F50, F73, F138-F162, F180, F181, F254-F292.

**Date and tip.** 2026-11-01, `experiment/di-redesign` at `331c35b9`. "At HEAD" means that branch.
Where master matters it is named: rows 31-51 were built and merged on master (`#373`-`#393`, merge
`861e9840`), so master holds the old framework with the DI build applied and ADRs 0057-0059
(`docs/adr/0057-the-container-answers-or-refuses.md`, `0058-a-provider-declaration-is-a-value.md`,
`0059-every-key-is-a-type.md`, first at `ad3302d9` and `8668cd70`); the rebuild branch carries none of
the three ADRs and none of that code.

**Method.** Each status was decided by reading the rebuild: `crates/ulo/src` (graph, binding,
dependency, module, lifecycle, error), `crates/ulo-macros/src` (injectable, module_attr, shared),
`crates/ulo-db/src/module.rs`, the core tests under `crates/ulo/tests`, core `DESIGN.md`,
`DIVERGENCES.md` and `SWITCH_INVENTORY.md`. Nothing was compiled or run. Every probe the source
documents cites (`fable-inject/`, `fable-visibility/`, `row50/`, `macroprobe/`, `fable2/` under the
session scratchpad) is gone from disk, so no probe was re-run; each probe's claim is checked against
the rebuild by reading.

Status words: **built** (the rebuild contains it), **superseded** (the problem is solved another way
or its object is gone), **open** (still true at HEAD, or unanswered), **dropped** (no longer applies),
**UNVERIFIED**.

---

## 1. Summary

| Status | Items |
| --- | --- |
| built (including "built differently" and "built as decided") | 115 |
| superseded | 55 |
| open | 7 |
| dropped (including "declined") | 41 |
| UNVERIFIED | 9 |
| total | 227 |

Counted over every item row of §2-§7, the probe table included, with A-oos's six parked threads
counted one each. Seven of the nine UNVERIFIED items belong to other subjects' readers (§9).

The rebuild is a DI redesign, so most items are built or superseded. What survives is concentrated
in four places: a finding class that recurs on new code (F152), two decisions the rebuild took the
other way without carrying the argument that was made against them (import cycles; tier shadowing),
one finding the rebuild did not reach (F256), and a handful of specified details the rebuild dropped
(`Debug` on `Dep`, the remedy lines of the ambiguity report).

### The five most valuable open items

1. **F152 recurs in `ulo-db`'s `StoreModule`** (C.F152). The identity is the options alone; `lazy`,
   `without_health_check`, `retries`, `backoff`, `connect_within`, `migrate` and `migrate_within` sit
   outside it, and the registry keeps the first module it reaches under an identity with nothing
   reported. Two imports of one store differing only in a knob start with whichever the walk reached
   first. The ledger's F152 names the deleted `CheckedModule`; the new site is not filed.
2. **Module import cycles are refused at HEAD** (I.V-f). `MODULE_VISIBILITY_REVIEW.md` §2f rejected
   that refusal with a stated argument; the rebuild's `register.rs` records every cycle as
   `WiringError::ImportCycle`. `SWITCH_INVENTORY.md` Q3 asks whether it is intended; the argument
   against it is not in the rebuild's record.
3. **F256: a collection cannot take an existing binding** (F.F256). `into dyn Plugin: [Alpha]`
   beside `providers = [Alpha]` builds two `Alpha`s, and under shared instances (D14) the plugin
   injected directly and the one in the collection are different objects.
4. **A factory's output never runs its trait hooks, and nothing says so at compile time**
   (C.F139). The rebuild states the rule (DESIGN §9.1) and gives `provide_with`; a type with
   `impl OnModuleInit` bound through `m.singleton(..)` compiles and its hook never fires. This is
   F139's class, which the audit put in one sentence: "The contract itself is stated nowhere." It is
   stated now and still silent at the site.
5. **`Dep`, `Many` and `Ext` have no `Debug`** (G.2.1). The core model specified a `Debug` that
   writes the key, so that a struct holding `Dep<dyn Repo>` derives `Debug` without `Repo: Debug`
   (probe P6). The rebuild implements none, so `#[derive(Debug)]` on an injectable fails on any
   dependency field.

**Live defects at HEAD the ledger does not hold:** the `StoreModule` identity (item 1, the class is
F152, the site is new) and the missing `Debug` (item 5). Two smaller report defects, the ambiguity
report's wording (I.V-a) and the `Result`-bound factory with no hint (D.D9), are listed under §4.

---

## 2. Findings of the audit: F138-F162, and the older entries it touched

### A. F138-F162

| ID | What | Status | Evidence at HEAD | Value to extract |
| --- | --- | --- | --- | --- |
| F138 | A second provider under one token replaces the first; two `APP_GUARD` registrations run one guard twice | superseded | `DuplicateBinding` per module, `crates/ulo/src/graph/wire.rs:799-826`; global enhancers are contributions under role keys, DESIGN §7; tests `wiring_bindings.rs::duplicate_single_binding`, `single_beside_collection` | The probe result is the test idea for any global-enhancer surface: two registrations in one module and the same pair across two modules, asserting both run in declaration order (master gave `["GuardB","GuardB"]` and `["GuardA","GuardA"]`). No rebuild test asserts two global guards both run and in collection order; `enhancer_roles.rs` checks role detection only. |
| F139 | `provider_token!` drops the wrapped type's lifecycle hooks | open (class) | The macro is gone (superseded). The class survives: DESIGN §9.1: a `Cache` bound through `m.singleton(..)` by a factory "compiles and never runs" its `OnModuleInit` impl | See below. |
| F140 | Hooks fire in hash order; shutdown is not the reverse | built | `crates/ulo/src/graph/order.rs:44-99` (stable topological sort); DESIGN §9.2; `hooks.rs::hooks_run_in_dependency_order_and_shutdown_in_reverse_for_a_chain`, `hooks_in_a_diamond_follow_every_edge_and_shutdown_is_the_exact_reverse` | The audit's flip test (add one unrelated provider, the order of two existing hooks changes; reproduced with four names) is not in the rebuild suite. A test that adds an unrelated binding and asserts no reorder pins the property directly. |
| F141 | `ModuleRef::…global()` reaches any provider in any module | superseded | `ModuleRef::get` reads its module's visibility only, `crates/ulo/src/module/handle.rs:34`; no widening method exists | D1's two meanings of `global` (visibility, application) and "search is not a third" are the rule the rebuild keeps. |
| F142 | A token two modules hold resolves to an arbitrary one, at four sites | built | `Visible::Ambiguous` per table, `graph/visibility.rs:97-128`; reported per read, `:358-367`; runtime `LookupError::Ambiguous`, DESIGN §8.2; `wiring_visibility.rs::two_imports_export_one_key` | Its fourth site (the role registry, F271) is answered by roles being collections. |
| F143 | A `Token<T>` const is nameable only as a bare single-segment path; each surface accepts a different subset | superseded | Keys are types and qualifier types, `crates/ulo/src/key.rs`, DESIGN §3.1; no strings and no consts | The six-column spelling matrix (§4 of the memo) is the record of why: one heuristic, `is_const_identifier`, decided type-or-const by casing and segment count. Lesson kept by the rebuild's principle 5: "stop reading the spelling". |
| F144 | `#[module]`'s four keys parse four grammars | superseded | `crates/ulo-macros/src/module_attr/args.rs:12-22` (`imports: Expr`, `controllers: Type`, exports `Type` with `@ Q` and `reexport`) | — |
| F145 | `init = "from_request"` is a magic string selected by a constructor's name | dropped | No `init` key; the constructor is `new` or `#[construct]`, `injectable/impl_form.rs:67-99` | The general rule, kept by the rebuild: never branch on the name of a user's item. |
| F146 | A one-module cycle names one token; the chain diagnostic serves only the cross-module case | built | `graph/cycles.rs:27-51`, one cycle per strongly connected component, each step with its module | — |
| F147 | `UloApplication::new` is `pub`, takes a private type, and keeps `Container` inside the `unreachable_pub` gate | superseded | `App` typestates (DESIGN §2); public reach is held by `scripts/public_reach.py` (DESIGN §0.7); no `unreachable_pub` in `crates/ulo/src/lib.rs` | The probe result: a `pub(crate) use` re-export does not suppress `unreachable_pub`, and a public signature naming a private type is what keeps that type "reachable" for the lint. Worth knowing if the gate is ever added. |
| F148 | `provide!` classifies its token twice | dropped | No `provide!` | — |
| F149 | The phase-1 retry budget is unreachable | dropped | `wire()` is one pass with no retry loop, DESIGN §10.1 | — |
| F150 | `Arc<Box<dyn Provider>>` through the SPI | dropped | No `Provider` SPI; no `Arc<Box<dyn` anywhere under `crates/ulo/src` | — |
| F151 | Four in-source documentation defects | dropped | The files are gone | — |
| F152 | A startup check is outside module identity; two imports differing in the check dedup by scan order | open | Recurs: `crates/ulo-db/src/module.rs:58-69` (fields), `:203-209` (identity over options only); first-reached wins in `crates/ulo/src/graph/register.rs:145-151` | See below. Not in the ledger at its new site. |
| F153 | A dispatch target cannot inject a collection; `create` panics | superseded | One `FromContainer` path for every injection point, `crates/ulo-macros/src/shared/dependencies.rs`; `Many<T>` | — |
| F154 | A factory over an execution-scoped dependency panics out of `create` | built | Needs-execution pass, DESIGN §6.2, `graph/scopes.rs`; `ScopeViolation` | The halt behind it (Halt 7) is the lesson: a refusal needs an error channel in the SPI, and a panic net around `build` catches the wrong set (nothing under `panic = "abort"`, and a user's own `expect` turned into a startup error). The rebuild's `Construct::construct` returns `Result` and catches panics as `FailureReason::Panicked`; both halves of that argument hold. |
| F155 | Two 163-line resolvers differ in two substitutions | dropped | No generated resolvers; `FromContainer::from_container` per type | The measurement rule stays useful: count differing lines after substituting the noun, with an extractor asserted to return whole bodies. It refused the fix it was run to support (the controller's resolver was 95% different). |
| F156 | `#[injectable]` and `#[controller]` disagree on `init`; the refusal says they agree | dropped | No `init`, no `#[controller]` | — |
| F157 | `#[new]` and `init` read dependencies from different places | superseded | The impl form reads the constructor's parameters, the struct form every field, `injectable/impl_form.rs`, `struct_form.rs` | — |
| F158 | `#[default(expr)]` is inert beside `#[new]` | superseded | The struct form has `#[injectable(default)]` only (`struct_form.rs:79-102`); the impl form's constructor sets every field; one placement per type | — |
| F159 | Four construction mistakes report against macro internals | built | Span errors naming the user's items, `impl_form.rs:74-124` ("a second #[construct] fn", "no constructor: name it `new`, or mark the fn…") | — |
| F160 | `ExtensionFactory<T>` and `ExtensionsFactory` differ by one letter | dropped | Both gone; `Ext<T>` needs no registration | — |
| F161 | Two declaration vocabularies over ten code paths | superseded | One value API (`ModuleDef`, `Contribute`) and one macro DSL that lowers to it, DESIGN §0.2, §4 | — |
| F162 | Four names for two acts, two accessors for one | superseded | `also_as`, `alias::<T, Q>().of::<E>()`, `contribute`, `provide::<T>()`, `crates/ulo/src/module/def.rs`, `binding/alias.rs` | — |

**F139, value to extract.** The audit counted four holes in one contract, each found separately
(F139, DOCS_AUDIT A8, A9, C17), and named the defect as the absence of a statement: "no doc says
which registration forms preserve hooks." The rebuild closes A9 and C17 by compile-time and wiring
refusals (`HookCapable`, DESIGN §3.5, §6.2) and states the factory rule in §9.1, with `provide_with`
as the route that runs `T::hooks`. The remaining case compiles and never fires: a type implementing a
hook trait, bound through a plain factory. The `#[module]` macro and `ModuleDef::singleton` see the
factory's output type at a concrete call site, which is where an autoref probe can tell whether that
type implements `OnModuleInit` (the same mechanism `#[injectable]` uses to fill `hooks`, DESIGN
§3.4). A probe there could refuse, or point at `provide_with`. Proposed as a question in §4.

**F152, value to extract.** The audit's finding: `CheckedModule` delegated identity to its inner
module, whose hint was the connection string, so `for_root(url)` and `for_root(url)
.without_startup_check()` rendered one key, and the container kept the first. The rebuild's
`StoreModule<S>` repeats the shape: `identity()` hashes `Identity::Options(options)` or
`Identity::Failed(path, error)` (`ulo-db/src/module.rs:186-209`), and the struct carries seven
settings beside the options (`:58-69`). `Registrar::enter` returns the existing node for an identity
already registered and never runs the second module's `register` (`register.rs:145-151`), so a
second import with `.lazy()` or `.without_health_check()` or another `.migrate(..)` is dropped with
no report. DESIGN §3.6 says "the same configuration imported twice is one module [31]", which is
right for an identical value and silent for a value that differs outside the identity. The audit's
two candidate fixes carry over unchanged: fold every setting into the identity (two modules with two
pools, refused where their exports clash), or refuse two imports of one identity whose settings
differ, naming both import sites. A test: two imports of one `StoreModule` with the same options,
one `.lazy()`, asserting `wire()` reports or that both instances exist.

### B. Older entries the audit cleared, cited or listed as filed

| ID | What | Status | Evidence at HEAD | Value to extract |
| --- | --- | --- | --- | --- |
| F37 | Module instantiation retried on a substring of an error message | dropped | Cleared by the audit on master (`LoadError::Deferred`); the rebuild has no deferral loop | — |
| F6 | String-anchored identity collides across two versions of one crate | built | Keys compare on `TypeId`; a missing key that matches a bound key's full name on another `TypeId` gets the hint "the same type from two versions of its crate; `cargo tree -d`", `graph/visibility.rs:441-445`, `error/wiring.rs:645` | — |
| F7 | Injecting a dispatch target reads as a `Clone` complaint | built | No `Clone` requirement (DESIGN §5, "Services are never cloned"); `ControllerRead` refuses a provider reading a controller, `graph/visibility.rs:230-295` | — |
| F31 | Two unnamed Prisma clients replace each other | dropped | No Prisma crate in `crates/` | — |
| F38 / F80 | Clippy in no CI job (one finding filed twice) | superseded | `.github/workflows` runs `cargo clippy … -D warnings` | The duplicate filing is still in the ledger; outside this subject. |
| F43 | Three names resolve to two things by path | UNVERIFIED | Cited as already filed; names a non-DI surface | Outside this subject; listed so it is not lost. |
| F44 | Nineteen unreachable public items (DI half) | superseded | `scripts/public_reach.py`, DESIGN §0.7 | — |
| F45 | Three `TypeId`-keyed maps written three times | dropped | The container that held them is gone | — |
| F50 | `#[use_guards(APP_GUARD)]` does not compile | dropped | No `APP_*` consts; global enhancers are `into AnyGuard<Http>: [..]`, DESIGN §7 | — |
| F73 | Four files under `di/` serve one transport | superseded | The core is generic over `Transport`; HTTP middleware is `PreDispatch` in `ulo-http` (transports §3.3) | — |
| F16 | One struct cannot serve two proto services | UNVERIFIED | gRPC; listed in the handoff's already-filed set | Outside this subject. |
| A8 | `lifecycle` flag on `provider_value!`/`provider_factory!` does not compile | superseded | Closure hooks on a typed binding handle, DESIGN §9.1 | — |
| A9 | A hook on an execution or transient provider compiles and never runs | built | `Construct<Scope: HookCapable>`, DESIGN §3.5; refused at `wire()` for an `Auto` type inferred per-execution, §6.2 | — |
| A27 | A scoped `provider_factory!` does not build once per execution | superseded | `m.execution(..)` and the execution cache, DESIGN §3.8 | — |
| C17 | A per-call target discards its init `Err` and never runs its shutdown hooks | built | An `Auto` controller with hooks that becomes per-execution is refused at `wire()`, DESIGN §6.2 table row 4 | — |
| C18 | `controllers:`/`exports:` take a bare `Ident` | superseded | `module_attr/args.rs:14-22` | — |
| C19 | Two by-type lookups bypass `token_of` | dropped | No string tokens | — |
| C20 | GraphQL crates hand-format two tokens | dropped | No string tokens | — |
| B23 | `extension.rs` says execution scope is HTTP-only | dropped | File gone; executions span four transports, DESIGN §6.3 | — |

### C. The audit memo's other claims

| ID | What | Status | Evidence at HEAD | Value to extract |
| --- | --- | --- | --- | --- |
| A-mech-1 | Three defects are one mechanism: an ambiguous question answered by whatever a hash map yields first | built | The rebuild answers by refusal or by a stated order at every such site: duplicates (`wire.rs:799`), visibility (`visibility.rs:117-121`), order (`order.rs`) | The diagnosis, as a review lens: wherever iteration order of an unordered map decides an answer, either the order is a contract and the storage must keep it, or the question is refused. |
| A-mech-2 | Two defects are one mechanism: a macro reads an identifier's spelling | built | Principle 5, DESIGN §0; macros emit `<#ty as FromContainer>` and never inspect the type, `shared/dependencies.rs:24-50` | — |
| A-sound-1 | `di::Execution` beside `ProviderScope::Execution` is deliberate | dropped | Neither type exists | — |
| A-sound-2 | `imports()` evaluated lazily at scan is what makes import cycles free; do not collapse `ModuleMetadata` to a descriptor | superseded | `Module::register(&self, &mut ModuleDef)`, DESIGN §3.6; import cycles are refused (see I.V-f) | The argument survives as the case against refusing import cycles: an import edge records no construction dependency. |
| A-sound-3 | Two modules declaring one provider type build two instances, and that is correct | built | `wiring_bindings.rs::qualifiers_and_modules_keep_equal_types_apart` | — |
| A-sound-4 | Module construction order is correct at both levels | superseded | One order, `order.rs:14-41` (post-order over imports) refined by `:44-99` | — |
| A-oos | Six parked threads | see rows | `injectables-by-value` (D14, built); `multi-provider-redesign` (built, `Many<T>`); `module-as-di-participant` (dropped: a module is not injectable, `ModuleRef` is an injection point, DESIGN §8.5); `lazy-loading-modules` (built, DESIGN §8.6); `behavioral-traits-audit` (UNVERIFIED, no artifact to check); `provider-context-variant-criterion` (dropped, `ProviderContext` gone) | — |
| A-perf | The `Arc<Box<…>>` hop, `IdHasher`, linear scans left to the performance pass | dropped | The structures are gone | — |
| A-book-1 | F38 and F80 are one finding filed twice | open (ledger) | Both entries still in `FRAMEWORK_GAPS.md` (:2154, :2463) | Ledger housekeeping only. |
| A-book-2 | Workspace `CLAUDE.md` names `ProviderContext`/`provider_scope` after their rename | dropped | The workspace `CLAUDE.md` describes the master framework throughout; correcting it is a switch task, not a DI one | — |
| A-meth | Confidence marking: three probes measured the author's own mistake before a control caught it (`Body::text` ambiguity read as a refusal; a classifier thought reachable after `token_type_to_tokens` discarded it; a two-door factory that compiled a provider of futures) | built (practice) | The rebuild's `probes/` keeps `*_fails.rs` binaries whose compile error is the result (README) | The three cases are the record of why every probe runs beside a control that must pass and one that must fail. |

---

## 3. Decisions D1-D14

| ID | Decision | Status | Evidence at HEAD | Value to extract |
| --- | --- | --- | --- | --- |
| D1 | `.global()` narrowed; renamed `.or_global()`, then revised to `.visible()` reading own, imports' exports, globals | superseded | No widening method; `ModuleRef::get` reads the module's table (`module/handle.rs:34`). The tier order the revision kept was dropped: own beside an import's export is `Ambiguous` (`visibility.rs:97-121`, DIVERGENCES "No shadowing" C1) | The two-meanings-of-`global` statement. The naming argument for `.visible()` ("get the visible `T`" reads right for every tier) has no object at HEAD. |
| D2 | A key is single-bound unless declared multi; a second single binding is refused naming both; `APP_GUARD` becomes a collection | built | `wire.rs:799-826` (per module), app-wide kind mix `:827-890`; `contribute` | The argument for refusing rather than last-wins: last-wins as an override cannot express "two global guards", and nothing documents it. |
| D3 | Construction is dependency order, ties by declaration order (depth-first over the declaration list), hooks follow, shutdown reverses | built (differently) | `order.rs:44-99` pops the smallest ready `BindingId` (module post-order, declaration index); DESIGN §9.2 | See below. |
| D4 | One token classifier: any path, the compiler decides | superseded | No token positions; keys are types | — |
| D5 | `#[module]` export parity with the builder | superseded | `module_attr/args.rs`; exports are types with `@ Q` and `reexport` | — |
| D6 | `Arc<dyn Provider>`; an SPI break is allowed in any phase | dropped | No provider SPI | The standing answer (SPI breaks allowed pre-release) is D13's rule restated. |
| D7 | Sequencing belongs to a triage pass | dropped | The sequence paused 2026-10-01 | — |
| D8 | The declaration surface becomes values; five proc macros go | superseded | Value API plus one DSL that lowers to it; Halt 4 already reversed D8's letter on master (one `provide!` macro) | Halt 4's rejected list is the useful record (see E). |
| D9 | A provider factory is always async | built | Value API factories take closures returning futures (`ModuleDef::singleton`, DESIGN §4); `#[guards]`/`into` wrap a sync body in `async move` | See below. |
| D10 | `#[new]` is the only constructor | superseded | `new` auto-detected or `#[construct]`, impl form, `injectable/impl_form.rs:67-99`; `#[construct]` outside an `#[injectable]` impl is an error (`ulo-macros/src/lib.rs:32-37`) | — |
| D11 | Naming: `Registration`, `T::provide()`, `.under`, `.multi`, `.or_global` | dropped | None of the names exist; the rebuild names by its own rule (DESIGN §0.3) | The rule D11 applied, "a type is named for what its consumer does with it", is close to the rebuild's §0.3. |
| D12 | One provider method on the builder; the attribute and the builder take the same expressions | built (differently) | `#[module]` lowers to `ModuleDef` calls (DESIGN §4, "This is exactly what the macro expands to") | — |
| D13 | Pre-release: no deprecation shims, no migration errors | built | No aliases or migration errors for the master spellings in `crates/ulo-macros/src` | The distinction it drew stays useful: the rule is about keeping an API alive, not about a diagnostic explaining a rename. |
| D14 | An injectable is shared: `Arc` for singleton and execution scope, by value for transient; no `#[derive(Clone)]` | built (differently) | `Dep<T>` is an `Arc` for every scope, transient included (fresh `Arc` per site), `dependency/dep.rs`; `scopes.rs:36-49`, `:96`, `:124-125`, `:213-221` | See below. |

**D3, value to extract.** D3 specified *depth-first over the declaration list*: walk the list and,
before emitting a provider, emit everything it injects. The rebuild's Kahn walk takes the smallest
ready id. Both give back a module written in a valid order exactly as declared, which was D3's
stated test. They differ when a provider is declared before its dependency: for `[A, B, C]` with
`A` reading `C`, D3 gives `C, A, B` and the rebuild gives `B, C, A`. Both are topological and stable.
D3's form keeps a dependency adjacent to its first dependent, which is the order a reader of the
declaration list predicts. Separately, the core model said a module's own hook runs ahead of its
providers' (§4); the rebuild runs it after them (DESIGN §9.2,
`hooks.rs::a_module_hook_runs_after_its_providers_hooks_and_before_the_next_module`). Neither
difference is a defect; both are decisions with no recorded reason in the rebuild.

**D9, value to extract.** Three items:

- The general rule, worth keeping in the rebuild's principles: **Nest can accept a value or a promise
  from `useFactory` because JS `await` on a non-promise is a no-op; Rust has no such affordance, and
  copying a duality means building the affordance by hand.** The DI build handoff put it as "where a
  language affordance is being copied rather than a design, check the affordance exists."
- The two rejected forms, each probed: a marker type parameter over sync and async is `E0282` at the
  call for an async closure; two entry points compiled a provider of futures when the async form
  went through the sync door, until an `Output: Clone` bound was added. The rebuild meets the same
  shape on fallibility and resolves it the same way at the macro: `singleton`/`try_singleton` are two
  doors in the value API, and `#[module]` picks the arm by autoref over the closure's own type
  (DESIGN §4). The hazard of the second door is live at the value API: `m.singleton(|| async {
  PgPool::connect(..).await })` binds `Result<PgPool, sqlx::Error>` (`ModuleDef::value` and the factory
  forms bind "whatever type", `module/def.rs:176-181`). A reader of `Dep<PgPool>` then gets "missing
  dependency `PgPool`" with no hint, because `spelled_alike` compares last segments after stripping
  auto traits (`visibility.rs:433-439`) and `Result<PgPool, Error>` is not spelled like `PgPool`. A
  near-spelling case for "a binding of `Result<T, _>` exists" would name the fix (`try_singleton`).
- The implementation note: `AsyncFn` as a bound accepts every async spelling and rejects sync cleanly,
  but its future type is unstable, so `Send` cannot be named on it; the stable form is
  `F: Fn(A..) -> Fut, Fut: Future<Output = R> + Send`. The rebuild's `execute` uses `AsyncFnOnce`
  where no `Send` bound is written (DESIGN §6.3), which is the one place that is sound.

**D14, value to extract.** Four things the rebuild did not carry in writing:

- The framing: "what is wrong today is not a cost, it is a promise." Scopes were read as statements
  about identity and were statements about construction count; the `Arc` existed in both layers and
  was discarded at the boundary. The rebuild's DESIGN §5 states the result ("Services are never
  cloned") without the reasoning.
- The rejected alternative, reached again by the user on 2026-09-30 (Halt 8's field-spelling
  question): a field typed `T` cannot be filled by a container without moving the one instance out,
  cloning it, or having the macro rewrite the type to `Arc<T>`, which surfaces in a hand-built struct,
  a constructor taking `T`, and an error naming `Arc<T>`. This is the argument for `Dep<T>` at every
  site and it is not in DESIGN.
- The transient change. D14 kept transient by value; the core model's open decision 3 chose `Dep<T>`
  over a fresh `Arc` so that no plain-site machinery returns for one scope; the rebuild built that.
  `Arc::get_mut` on `into_arc()` gives `&mut` for a sole holder.
- The verification, built: three `ptr_eq` assertions (singleton shared, execution-scoped shared within
  an execution, transient not shared) are in `scopes.rs`, and a non-`Clone` singleton shared by two
  holders is `scopes.rs:36` and `injection.rs:327`.

---

## 4. The build handoff and the DI rows of `BUILD_SEQUENCE.md`

### D. Handoff constraints, ADRs and PR rows

Row numbers are `BUILD_SEQUENCE.md`'s; PR numbers are the handoff's. Rows 31-51 merged on master.

| ID | What | Status | Evidence at HEAD | Value to extract |
| --- | --- | --- | --- | --- |
| H-c1 | No DI defect leaves `create` by panic | built | Panics in user code are caught and reported as `FailureReason::Panicked`, DESIGN §14.5; wiring errors are values, §10.1 | — |
| H-c2 | Pre-release: no shims (D13) | built | see D13 | — |
| H-adr1 | ADR "the container answers an ambiguous question, or refuses it" (D1-D3) | superseded | On master as ADR-0057; not on this branch; DESIGN §8.2, §9.2, §10.1 carry the content with no-shadowing changed | The ADR text is reachable at `git show master:docs/adr/0057-the-container-answers-or-refuses.md`. |
| H-adr2 | ADR "a provider declaration is a value" (D4, D8-D12) and its rule for D9 | superseded | On master as ADR-0058 and ADR-0059 (every key a type) | `git show master:docs/adr/0058-a-provider-declaration-is-a-value.md`, `…/0059-every-key-is-a-type.md`. 0059's research summary (Guice, Dagger, Spring, .NET keyed services and `more-di` key by type plus qualifier; typemap's `trait Key { type Value; }`) is the provenance of the rebuild's `Key::of::<T, Q>()`. |
| R31 | DI-ADR | superseded | as H-adr1/2 | — |
| R32 | DI-1: value surface, `Provide::*`, `.under`/`.multi`, `IntoFactory` per arity | superseded | Value API with `Factory<Args>` per arity (`binding/factory.rs`) | Halts 3 and 4 (E). |
| R33 | DI-2: migrate call sites, delete the macros | dropped | Nothing to migrate on this branch | — |
| R34 | DI-3: module grammar | superseded | F144 row | — |
| R35 | DI-4: `#[new]` only | superseded | D10 row | — |
| R36 | DI-5: `#[default]` beside `#[new]` refused | superseded | F158 row | — |
| R37 | DI-6: hooks that cannot fire refused | built | A9, C17 rows | — |
| R38 | DI-7: multi as token semantics, duplicates refused | built | D2 row | — |
| R39 | DI-8: insertion-ordered maps, ordering contract | built | F140 row | — |
| R40 | DI-9: widening lookup narrowed, ambiguity refused | built (differently) | F141, F142 rows; I.V-a | — |
| R41 | RPC-7: two controllers on one pattern refused | UNVERIFIED | RPC subject, F226 | Outside this subject. |
| R42 | DI-10: collection branch shared with the controller resolver | superseded | F153 row | — |
| R43 | DI-11: twin resolvers collapsed | dropped | F155 row | — |
| R44 | DI-12: factory over an execution-scoped dependency refused | built | F154 row | Halt 7 (E). |
| R45 | DI-13: `Injectable` renamed `Registration` | dropped | D11 row | — |
| R46 | DI-14: `UloApplication::new` private; `ModuleRef` lookups through `token_of` | superseded | F147, C19 rows | — |
| R47 | DI-15: startup check joins module identity | open | F152 row: recurs in `StoreModule` | — |
| R48 | DI-17: retry budget deleted; per-module cycle through the chain diagnostic | built | F146, F149 rows | — |
| R49 | DI-16: `Arc<dyn Provider>` | dropped | F150 row | — |
| R50 | DI-19: an injectable is shared | built (differently) | D14 row | — |
| R51 | DI-20: `ExtensionFactory<T>` deleted, the container answers `Extension<T>` from the bag | built | `Ext<T>` read by the injection point, `dependency/ext.rs`; DESIGN §3.2 | Halt 9 (E). |
| R52 | DI-18: in-source docs, `ExtensionsFactory` rename, the `Execution` variant criterion | dropped | Never built on master; the files are gone | — |
| R53 | DI-21: `Request` loses two precomputed maps | dropped | Never built; no `Request` provider | — |
| R54 | DI-22: `Request` deleted; `RequestPart` a provider and an extractor | superseded | `RequestHead` is an execution input seeded by HTTP, `crates/ulo-http/src/transport.rs:49`, DESIGN §6.4 | F180's finding that the framework parsed the query string four times with three behaviours (F117) belongs to the HTTP/extraction readers. |
| H-ver | The per-PR verification list (handoff "Verification each PR needs") | built (mostly) | Rebuild tests cover duplicates, single/collection mix, hook order and reverse, reexport, scope refusals, sharing (`wiring_bindings.rs`, `hooks.rs`, `wiring_modules.rs`, `wiring_scopes.rs`, `scopes.rs`) | Two tests in the list have no rebuild counterpart: PR 7's "two global guards in one module and across two modules both run, in declaration order", and PR 8's "adding an unrelated provider changes no hook order (at least two names)". Both are cheap and pin properties the rebuild claims. |
| H-proc | The divergence gate, the Law review, the per-PR checklist | dropped | Process for a build that is not running | The divergence definition is reusable: a signature that differs, a rejected alternative turning out necessary, a verification that cannot be written as stated. |

### E. Halts, decisions and the pause

| ID | What | Status | Evidence at HEAD | Value to extract |
| --- | --- | --- | --- | --- |
| Halt 3 | A generic value constructor cannot detect an enhancer role; answer: enhancers are types, a token can carry a role | superseded | A role comes from the key, decided by `TypeId` at freeze (DESIGN §3.7, §7, §14.18); `#[guards(T)]` checks `T: Guard<Http>` by bound | See below. |
| Halt 4 | `.multi::<dyn Tr>()` needs `Unsize`; answer: one `provide!` over the value API, multi by element type | superseded | `contribute::<dyn Plugin>().provide::<T>(coerce)`, the coercion written as an identity closure (`binding/contribute.rs:118-140`); `Many<T>` | See below. |
| Dec-handle | A handle type injects by value (`Shared` marker), amending D14 | dropped | Halt 8 withdrew it; `Dep<T>` everywhere | Halt 8's two refusals are the record (below). |
| Halt 5 | A token const in `#[use_guards]`: a const and a type are both bare paths, in two namespaces | dropped | No token consts | The namespace argument: "a const lives in the value namespace and a non-unit type in the type namespace, and no expansion compiles for both". It is why a key must be one kind of thing, and it led to ADR-0059. |
| Halt 6 | Every key is a type | built | DESIGN §3.1, principle 5 | The costs it accepted and the rebuild inherits: a marker type per named slot, no runtime-chosen keys (a runtime set of names is one binding holding the set), a cross-crate slot needs its marker imported. The ADR review's ruling on strings: "adding one later is non-breaking, removing one is not." |
| Dec-alias | `alias` names a binding by the source rule | superseded | `m.alias::<T, Q>().of::<E>()` keeps the type and changes the qualifier, `binding/alias.rs:13-31`; a second key under another type is `also_as` | — |
| Halt 7 | A startup refusal needs an error channel in the SPI | built | `Construct::construct` returns `Result<Self, ConstructError>` (DESIGN §3.4) | Option 3's refutation, recorded in F154's row. |
| Halt 8 | The `Shared` amendment cannot be built | built (its answer) | `Dep<T>` at every site | See below. |
| Halt 9 | The container cannot answer `Extension<T>` it knows only as a string | built | `Ext<T>` is an injection point keyed by the bag (DESIGN §3.2); a factory parameter is an injection point too (`Factory<Args>` reads every parameter through `FromContainer`, closing F255) | — |
| Q-r40 | How far `.or_global()` reaches | superseded | I.V-a | — |
| Pause | The sequence stops for the redesign | built (as an event) | The rebuild on this branch | — |

**Halt 3, value to extract.** Probed and recorded: a generic function `Provide::value<V>(token, v)`
sees `V` only through its bounds, so an autoref probe inside it always takes the fallback; stable
Rust has no specialization to recover the concrete type. A `#[module]` DSL "sees text, not types" and
cannot detect a role either. The rebuild's answer, roles by key `TypeId` at freeze, sidesteps both;
the cost the rebuild accepts instead is stated in DIVERGENCES ("recognised by the `type_name` prefix
of the role trait objects, read from a private probe transport"; a compiler changing `type_name`'s
format would demote contributions). Halt 3's probe is the reason a value-level role probe is not an
alternative to that.

**Halt 4, value to extract.** The rejected list, each a form someone will propose again: the audit's
`.multi::<dyn Tr>(name)` (generic `Arc<V>` to `Arc<dyn Tr>` needs the unstable `Unsize`;
`fn multi<Tr: ?Sized>(..) -> Arc<Tr> { Arc::new(v) }` fails `E0308`/`E0277`); a per-trait
`collection!(dyn Plugin)` line; a `tokens!` block with a marker per collection; typed consts plus a
`collects!` line per trait; and a no-declaration value API (`Arc::new` at every value, a cast in every
factory). The rebuild chose the coercion closure per call at the value API (`|a| a`, rejected here as
noise) and hides it behind `#[module]`, which writes it. That trade is reasonable; the record shows
why there is no third way on stable Rust.

**Halt 8, value to extract.** Two compiler refusals, probed on rustc 1.98.1: (1) dispatch telling an
`Arc<X>` field from a plain one, `impl<T: ?Sized> X for Arc<T>` beside `impl<T: Shared> X for T`, is
`E0119` once core implements `Shared for Arc<T>`, because `Arc` is not `#[fundamental]`; (2) the
integration crates cannot implement a core marker for `sqlx::Pool`, `DatabaseConnection`,
`ConnectionManager`, `mongodb::Database` or the deadpool pools: both are foreign, `E0117`. Any future
"let a handle type inject as itself" proposal meets both. The core model records the same wall from
the other side (§2.1: a `Handle` marker is `E0119` beside `Arc<T>` and `E0117` for a foreign pool).

### F. Findings filed by the DI build's reviews (F254-F285)

| ID | What | Status | Evidence at HEAD | Value to extract |
| --- | --- | --- | --- | --- |
| F254 | A value-API module carries no identity hint; two configs collapse | superseded | `DynamicModule::new::<Owner>(cfg, ..)` and `ModuleIdentity::of_owner`, DESIGN §3.6, §8.4 | — |
| F255 | A factory parameter `Arc<dyn T>`/`Vec<Arc<dyn T>>` is keyed by its own type | built | Factory parameters are `FromContainer` injection points, DESIGN §5; `bindings.rs::a_factory_s_parameters_are_its_dependencies_and_its_output_the_binding` | — |
| F256 | `into` from a type builds a second instance; a collection takes no existing binding | open | `Contribute::provide::<T>` constructs its own instance, `binding/contribute.rs:118-140`; `value`/`with` take a value or a fresh factory output; no form contributes an existing single binding | See below. The ledger entry exists and cites master's code. |
| F257 | A value's own `Provider` hooks have no declaration form (`RpcClient` never connected or closed) | superseded | Closure hooks on a singleton handle (`.on_init`, `.on_destroy`), DESIGN §9.1; `provide_with` runs `T::hooks` | Whether the RPC client uses them is the RPC reader's. |
| F258 | Only a keyed factory can be rescoped | built | `with(singleton \| execution \| transient)` in every position, DESIGN §3.3 table; `Contribute::execution`, `contribute.rs:251` | — |
| F259 | No lookup reaches a slot holding a trait object | built | `get<T: ?Sized>` answers `Dep<T>`, `module/handle.rs:34`, DESIGN §3.8 | — |
| F260 | A single binding and a collection under one trait object collide; a collection field panics | built | Kind mix refused app-wide, `wire.rs:827-890`; `Dep` and `Many` read different kinds | — |
| F261 | Two gateways at one path replace each other | UNVERIFIED | WebSocket subject | Outside this subject. |
| F262 | A `#[new]` parameter `Vec<Arc<dyn T>>` panics | superseded | `Many<T>` through `FromContainer` | — |
| F263 | Dead bookkeeping and stale comments in `ulo-macros` | dropped | Crate rewritten | — |
| F264 | A `#[new]` or hook in a child or sibling module is ignored | superseded | The impl form puts the attribute on the impl itself; hooks are trait impls found by probe over the concrete `Self` (DESIGN §3.4) | The probe result explains a class: an inherent const emitted without visibility is private to its module, so a read at the struct's site falls to a blanket default. Any future bridge between two macro sites has to state its visibility. |
| F265 | `#[inject]` beside `#[new]` is never filled | superseded | One placement per type | — |
| F266 | A `#[catch]` handler named bare fails startup with advice that does not apply | UNVERIFIED | Pipeline subject | Outside this subject. |
| F267 | A shutdown hook on a per-call controller compiles and never runs | built | C17 row | — |
| F268 | A collection field built before a contributing module drops its contributions | built | A collection read waits for every contribution: `Graph::construction_deps`, `crates/ulo/src/graph/mod.rs:316-327` | — |
| F269 | A field type from a `macro_rules!` `ty` fragment is refused | superseded | The macro emits the type opaquely, `shared/dependencies.rs:24-50` | — |
| F270 | Sibling modules at different import depths fire out of declaration order | built | Depth-first post-order over imports in written order, `order.rs:14-41` | — |
| F271 | An enhancer named by type resolves app-wide | built | Enhancer types resolve against the handler's module, `visibility.rs:189-199` | — |
| F276 | An `Arc<dyn T>` field over a collection-only key panics | superseded | `Dep<dyn T>` over contributions only is a missing single, reported at `wire()` | — |
| F277 | A transient of a per-call build resolved outside its execution | built | DESIGN §6.2 (transients pass the need upward), §3.8 | — |
| F278 | `ulo-ws-redis`'s factory panics on a Redis failure | UNVERIFIED | WebSocket integration | Outside this subject. |
| F279 | `Request` in a non-HTTP execution panics | built | `InputNotSeeded` at `wire()`, DESIGN §6.4, §10.1 step 5 | — |
| F280 | A controller reaching an execution-scoped provider through a transient is refused rather than elevated | built | Needs-execution passes upward through transients, DESIGN §6.2 | — |
| F281 | A collection gathers every module's contributions, exported or not | built (decided) | DESIGN §8.2 "Collections are the exception", §14.3 | — |
| F282 | A provider injecting a collection sees only contributions built before it | built | as F268 | — |
| F283 | A type alias hiding an `Arc` keys another slot | superseded | Reading by type: `type Store = Dep<Repo>` reads `Repo` | — |
| F284 | The plain-field refusal reads the type and refuses a plain field over a transient | dropped | No plain sites | — |
| F285 | A sized-element collection can be contributed and not injected | superseded | `Many<T: ?Sized>` holds `Arc<T>` for any `T`, `dependency/many.rs` | — |

**F256, value to extract.** On master the deleted `provide!("K", existing(Alpha), multi(Plugin))`
resolved the binding already registered; the replacement grammar dropped that. At HEAD the module
DSL's `into dyn Plugin: [Alpha]` lowers to `contribute().provide::<Alpha>(|a| a)`, a second binding
with its own construction, and `providers = [Alpha]` beside it is a first. Both are singletons, so
`Alpha::new` runs twice, and after D14 the two are observably different objects (`Dep<Alpha>` and
the item of `Many<dyn Plugin>`). Candidate, carried from the ledger and restated for the rebuild: a
contribution form that names an existing single binding, `contribute::<dyn Plugin>().of::<Alpha>(|a| a)`
or an `into` entry `alias Alpha`, the coercion closure carrying the `Arc<Alpha>` to `Arc<dyn Plugin>`
cast the alias would otherwise need `Unsize` for (Halt 4). Test: `providers = [Alpha]` plus `into dyn
Plugin: [alias Alpha]`, asserting one construction and `Arc::ptr_eq` between the direct read and the
collection's item.

---

## 5. The core model (`DI_CORE_MODEL.md`)

### G. Its sections

| ID | What | Status | Evidence at HEAD | Value to extract |
| --- | --- | --- | --- | --- |
| G.0 | The model in five sentences | built (differently) | Sentence 4 (tier precedence) reversed; the others hold, DESIGN §2 | — |
| G.1.1 | A key is a type; a second slot is a marker type implementing `Key { type Value }` | built (differently) | `Key::of::<T, Q>()`, the qualifier a second type parameter, DESIGN §3.1, §14.2 | The rejected list: Nest's three key kinds; a position rule telling a const from a type by casing; `Named<"db">`, unstable. The rebuild's qualifier needs no `Value` check because the type is written at the site (`Dep<PgPool, Replica>`). |
| G.1.1-dyn | `dyn Repo` and `dyn Repo + Send + Sync` are two keys; one spelling with the supertraits on the trait | built | `Dep<T>` requires `Send + Sync`; a missing key spelled like a bound one up to `+ Send + Sync` names both, `visibility.rs:433-455`, test `missing_dependency_whose_key_differs_only_by_send_and_sync_names_both_spellings` | Probe P5's result, a language fact that still holds: "there is no type-level way to strip auto-trait bounds from a `dyn` type", so a syntactic refusal is defeated by an alias and only the wiring report can catch it. |
| G.1.2 | Binding forms and rules: single unless multi; a role from a type; factory async; factory parameters are sites; `Extension<T>` needs no declaration; collections cross exports | built | DESIGN §4, §3.2, §8.2 | — |
| G.1.3-id | Module identity: base plus config fingerprint; later by type (F286) | built | `ModuleIdentity::of_type`/`of_value`/`of_owner`, DESIGN §3.6 | `TypeId` beside the name tells two versions of one crate apart, which `type_name` collapses; a fingerprint is still needed because `DbModule { url: a }` and `{ url: b }` are one type. Both are built. |
| G.1.3-vis | Visibility in tier order, nearer answers (Pv1-Pv3); direct imports only (Pv5a); re-export stalls (Pv5b); one refusal per `create` (Pv4) | superseded | Tiers refused (I.V-a); direct imports only holds (DESIGN §8.2); `reexport` (I.V-d); all errors in one pass (I.V-b) | — |
| G.1.4 | Three scopes as promises about identity; hooks refused on non-singletons at compile time | built | DESIGN §3.3, §3.5 | — |
| G.2.1 | `Dep<T: ?Sized + Send + Sync>(Arc<T>)` with `as_arc`, `into_arc`, `Deref`, `Clone`, `From<Arc<T>>`, a `Debug` writing `Dep<type_name>`; no `Default`, `PartialEq`, `Serialize` | built (with a loss) | `dependency/dep.rs:17-61`: `from_arc`, `into_arc`, `arc`, `Clone`, `Deref`; no `Debug` on `Dep`, `Many` or `Ext` (zero occurrences in the three files) | See below. |
| G.2.1-rej | Rejected: an enum `Dep { Shared(Arc<T>), Value(T) }` (cannot be `?Sized`, P1a); a `Handle` marker (E0119 beside `Arc<T>`, E0117 for foreign pools); a macro rewriting the field | built (as argued) | `Dep` is an `Arc` newtype | The three refutations stand as language facts. |
| G.2.2 | Every provider answers an erased `Arc<T>`; the plain-site machinery is deleted | built | No `Provider::shape`, no `SharedByValue`, no autoref ladder in the rebuild | — |
| G.2.3 | One `Site` trait: key, take, inject-key; reading by type | built (as `FromContainer`) | DESIGN §3.2: `describe` and `from_container`, implemented for `Dep`, `Many`, `Option<S>`, `Ext`, `ModuleRef`, `ExecutionRef` | — |
| G.2.4 | Lookups answer `Dep<T>`; `resolve::<Extension<T>>` stays unsupported | built | `get` answers `Dep<T>`; extensions through `exec.extensions()`, DESIGN §3.8 | — |
| G.3.1 | Struct form and impl form; `new` auto-detected; `#[construct]`; both forms on one type E0119 | built | `injectable/struct_form.rs`, `impl_form.rs`, `lib.rs:32-37` | — |
| G.3.2 | Roles from the type's trait impls at the concrete site | superseded | Roles by key at freeze (E, Halt 3) | — |
| G.3.3 | Factories with `Site` parameters | built | F255 row | — |
| G.3.4 | A third-party type enters by a factory or an integration's provider; the orphan rule keeps an application from implementing `Key` for `PgPool` | built | DESIGN §4 (factories), §13 (integration surface) | Probe P7 (orphan, E0117 for `Arc<App>` and `Dep<App>`) holds by the language. |
| G.4 | Lifecycle end to end | built (differently) | DESIGN §2, §9 (wire, connect, bind, serve, close) | — |
| G.5 | The refusal table | built (mostly) | DESIGN §12 | — |
| G.6 / F287 | Type-level wiring: each module carries `type Exports`/`type Imports`, each injectable `type Deps`, `Visible<T, Idx>` with an inferred index | dropped (declined) | DESIGN principle 1: one runtime container validated whole; the compiler checks what is local to one type | See below. |
| G.7-1 | Field helper naming | built | `#[injectable(default)]` (`struct_form.rs:79-102`); a qualifier is a type parameter, so no `#[inject(K)]` | — |
| G.7-2 | Collection spelling `Vec<Dep<T>>`; a `Deps<T>` newtype rejected as a second noun | built (differently) | `Many<T, Q>`, DESIGN §3.2, §14.1 ("they read differently and fail differently") | The rebuild took the alternative the model rejected, with a reason. `Dep<[T]>` stays impossible (unsized items). |
| G.7-3 | A transient answers a fresh `Dep<T>` | built | D14 row | — |
| G.7-4 | Halt 9's lookup half | built | G.2.4 | — |
| G.7-5 | F281 | built (decided) | F281 row | — |

**G.2.1, value to extract.** The model's `Dep` carried a `Debug` that names the key rather than the
value, `Dep<my_app::Repo>`, and probe P6 showed `derive(Debug)` failing over `Arc<dyn Repo>` and
passing over that `Dep<dyn Repo>`. The rebuild's `Dep`, `Many` and `Ext` implement no `Debug`, so a
user's `#[derive(Debug)]` on any struct holding a dependency fails, `Dep<Config>` included where
`Config: Debug`. Writing the key rather than the value also keeps a service's `Debug` from printing a
dependency's state, which may hold a secret. Three impls, each writing `Dep<{TypeName}>`
(`Many<..>`, `Ext<..>`); a test deriving `Debug` on a struct holding `Dep<dyn Trait>` for a trait with
no `Debug`.

**F287, value to extract.** The probes (P4a-P4d) are the reason the runtime graph is the one that
explains. A `Construct<R>` with `R: Resolve<T>` bounds is vacuous over one container type (P2a);
deriving "resolves what an import exports" per import is `E0119` with two imports (P4b); an index
parameter on the constructor's impl is `E0207` (P4d); the shape with teeth, `Visible<T, Idx>` with an
inferred index (frunk's `Selector`), reports a missing key as `E0277` (P4a) and an ambiguous one as
`E0283` "type annotations needed", which names no module and which `on_unimplemented` does not
reach (P4c). Naming both modules at compile time needs each `#[module]` to emit a `macro_rules!`
callback listing its exports, which reaches only macro-written modules and compares exports as token
text, the spelling problem type keys removed. A future "check the graph at `cargo check`" proposal
meets every one of these.

### H. Probes listed in the core model and the review

All probe sources are gone from the scratchpad. Each row says what the probe showed and whether the
claim still holds on the rebuild, by reading.

| ID | Shows | Status | Holds on the rebuild |
| --- | --- | --- | --- |
| P1a | A by-value-capable `Dep` enum cannot be `?Sized` (E0277 on `Dep<dyn Logger>`) | built | Language fact; the rebuild's `Dep` is an `Arc` newtype |
| P1b | `Dep<T: ?Sized>(Arc<T>)` is `Arc`-sized and holds a handle as `Arc<Handle>` | built | yes |
| P1c, P1d | One generic take cannot wrap a by-value answer; a sized impl beside an unsized one is E0119 | built | Language facts; the rebuild avoids both by answering `Arc` for every scope |
| P2a | `R: Resolve<T>` over one blanket impl is vacuous | built | The rebuild's `Construct` is not generic over the resolver (DESIGN §3.4) |
| P2c | Two front ends emitting one impl: E0119 | UNVERIFIED | The rebuild relies on it for struct form plus impl form on one type; the message a user sees needs a compile |
| P2d | A partial fake resolver is E0277; the one use the bound has, a test double, is given by an ordinary constructor | built | The impl form's constructor is an ordinary fn taking `Dep`s (`Dep::from_arc`) |
| P3a, P3b | One site trait serves fields and factory parameters; a plain site is E0277 with the note | built | `FromContainer` with `on_unimplemented`, DESIGN §3.2 |
| P4a-P4d | Type-level visibility (G.6) | dropped | Not attempted; the results stand as the argument |
| P5 | `dyn Repo` and `dyn Repo + Send + Sync` are two names | built | The wiring hint depends on it |
| P6 | `derive(Debug)` fails over `Arc<dyn Repo>`, passes over `Dep<dyn Repo>` | open | The rebuild's `Dep` has no `Debug` (G.2.1) |
| P7 | An application cannot implement a ulo trait for `Arc<App>` or `Dep<App>` (E0117) | built | Language fact |
| P8, P8b | The `Dep` model end to end; a non-`Send` trait object refused at the site | built | DESIGN §5 states the auto trait's own message, with no hint, as accepted |
| ulo-probe bins | A fallible or async `#[new]` fails E0271; `Option<Arc<T>>` fails `create`; `#[injectable]` on an impl is "expected `struct`"; a bare `new()` is ignored; a factory answering `Result` binds the `Result` | superseded (four), open (one) | The first four are built away (F288, F289). The last holds by design at the value API (D9 row) |
| Pv1-Pv3 | A key two tiers answer is answered by the nearer | superseded | Refused as `Ambiguous` |
| Pv4 | `create` names one refusal | superseded | `wire()` collects every error |
| Pv5a | A transitive export is not visible | built | DESIGN §8.2 |
| Pv5b | A re-export stalls `create` | superseded | `reexport`, `ExportNotBound` with the hint (`error/wiring.rs:458-464`) |

### Deferred entries the core model filed (F286-F289)

| ID | What | Status | Evidence at HEAD | Value to extract |
| --- | --- | --- | --- | --- |
| F286 | The `for_root` family names modules by string | built | `StoreModule<S>` identity `of_owner::<Self, _>` over options, `ulo-db/src/module.rs:203-209`, integrations DESIGN line 294 | Its new site carries F152 (C). |
| F287 | Compile-time module graph | dropped (declined) | G.6 | G.6. |
| F288 | The impl form | built | G.3.1 | The outside chat's two naming arguments the rebuild followed: `#[construct]` rather than `#[ctor]` (the `ctor` crate means "run before main") or `#[constructor]` (Rust has no constructors, only the `new` convention). |
| F289 | Fallible, async and optional constructors | built | DESIGN §4 (`async fn new(..) -> Result<Self, InitError>`), §3.2 (`Option<S>`) | The rule it set, kept: a fallible constructor's return is converted through an autoref pair, never by reading the return type's name, which an alias defeats (F240's lesson). The rebuild's `IntoConstructed` (`impl_form.rs:41`) is that conversion. |

---

## 6. The visibility review (`MODULE_VISIBILITY_REVIEW.md`)

### I. Policy points, decisions and filed entries

| ID | What | Status | Evidence at HEAD | Value to extract |
| --- | --- | --- | --- | --- |
| V-1 | The proposal's "one blanket impl, visibility as data" is the existing loader | built | `graph/visibility.rs` is that table, computed before construction | — |
| V-1-diff | Two unstated differences: the proposal refuses an ambiguous key whether or not anything injects it, and its resolver is pulled by the constructor rather than declared ahead | built (both, partly) | The root's table is swept whole (`visibility.rs:203-222`), other tables report only what a site reads; dependencies are declared ahead (`Construct::dependencies`, DESIGN §3.4) | The declared list is what makes a pre-construction check possible; the rebuild keeps it and enforces it at first construction (`UndeclaredRead`). |
| V-a | A local binding beside an import's export, and the globals: keep shadowing (recommended) or refuse across tiers | superseded | Refused: every source in one list, two or more `Ambiguous`, `visibility.rs:97-121`; DIVERGENCES "No shadowing" (C1) | See below. |
| V-b / F292 | Every miss and ambiguity in one pass, before any provider is built | built | `wire()` collects (DESIGN §10.1), no instances before `connect` | — |
| V-c | An explicit pick, `use_from::<K, M>()`: defer | dropped | Not built; qualifiers and `Keyed` modules cover the case (DESIGN §8.3) | — |
| V-d / F291 | Re-export: forward an `exports:` entry a direct import exports, refuse at scan one nothing provides | built (differently) | An explicit `reexport` keyword, `module_attr/args.rs:21`; `ExportNotBound` with the hint "re-export it with `reexport`", `error/wiring.rs:458-464`; `wiring_modules.rs::export_of_a_key_an_import_provides_says_to_reexport_it` | The review recommended forwarding a plain `exports:` entry; the rebuild requires the keyword and hints at it. Both close the stall. |
| V-e | Singleton cells in the owner; dedup by `TypeId` loses the fingerprint | built | DESIGN §3.6 | — |
| V-f | Module import cycles: the proposal refuses them; the review rejected that | open | Refused at HEAD: `graph/register.rs:145-151`, `graph/cycles.rs:12-17`, `wiring_modules.rs::an_import_cycle_is_refused_with_its_path`; `SWITCH_INVENTORY.md` Q3 and A2 row `import_cycle.rs` ask whether intended | See below. |
| V-g | `Module::register(&self, &mut ModuleBuilder)`: defer to F286 | built | `Module::register(&self, m: &mut ModuleDef<'_>)`, DESIGN §3.6 | The rebuild adopted the outside chat's shape. |
| V-3 | The `Resolve<T>`/`Construct<R>` bound is decorative for the graph and redundant for tests | built | DESIGN §3.4 "`Construct` is not generic over the resolver" | — |
| V-D1 | Tier order a rule or a defect | superseded | V-a | — |
| V-D2 | Re-export form | built | V-d | — |
| V-D3 | One-pass reporting now | built | V-b | — |
| V-oos | A testing override API (Nest's `overrideProvider`) is out of scope | built | `TestApp::override_value` and kin, DESIGN §11 | V-a's argument changes once this exists (below). |
| F290 | Tier order with nothing reported | superseded | V-a | — |

**V-a, value to extract.** The review argued for keeping lexical shadowing: the proposal's case,
"silent shadowing is how test mocks leak into production wiring", needs the mock declared in the
production module's own list, because a test root's binding does not reach into a module it imports;
and the override a test wants is a testing API, not a visibility rule. The rebuild refuses across
tiers and also built the testing API (`TestApp` overrides replace a recipe in place and must match
exactly one binding, DESIGN §11). With overrides in place, the review's second argument no longer
favours shadowing, and the refusal is consistent. What is missing at HEAD is the follow-through on
the refusal:

- No test pins it. `wiring_visibility.rs` pins two imports exporting one key and the root sweep; no
  test pins a module's own binding beside an import's export, or an import's export beside a global's.
  The review noted the same gap on master for the opposite rule ("local-over-import and
  local-over-global are pinned by nothing").
- The report mislabels and gives no remedy. `WiringError::Ambiguous` renders every source as
  "exported by M" (`error/wiring.rs:656-662`), including the module's own binding, which the module
  binds and need not export; and it carries no `help:` line, where the master refusal named two fixes
  (import one of the two, or bind one under a marker, `instance_loader.rs:741-744` at `95892262`). The
  rebuild's analogues are "import one", "give one a qualifier", "remove the local binding".

**V-f, value to extract.** The review's argument against refusing import cycles, which the rebuild's
record does not answer: an import edge is a visibility fact, not a construction edge. Under the
proposal's own model the visibility table computes over a cyclic import graph unchanged, since
visibility is a lookup; only construction order needs a DFS, and the provider graph has one, whose
cycle diagnostic names a real construction cycle when there is one. Master pinned the tolerated shape
(`integration-tests/tests/integration/import_cycle.rs:49`, `:105`,
`cross_module_dependency_through_import_cycle_resolves`: two modules importing each other, a provider
in one resolving a provider in the other, no provider cycle). The memory note
`project-circular-dependency-model` records the same: module import cycles tolerated, provider cycles
hard errors. The rebuild's registration walk claims an identity before `register` runs and records a
cycle when a module meets its own node (`register.rs:145-157`); its collection order skips the
closing edge and places every module once (`order.rs:11-41`), so the machinery to tolerate a cycle is
already there and the refusal is a choice. The rebuild's report gives one remedy, "move what the
modules in the cycle share into a module each of them imports" (`error/wiring.rs:425`), which is the
Nest-community answer and costs a module per cycle.

---

## 7. The outside chat's proposals (`OUTSIDE_CHAT_PROPOSALS.md`)

### J. Its items

| ID | What | Status | Evidence at HEAD | Value to extract |
| --- | --- | --- | --- | --- |
| O1-ctor | Constructor injection: parameters are the dependency list; plain fields set in the body | built | Impl form, DESIGN §4 | — |
| O1-impl | The attribute goes on the impl block, not the fn, because a method attribute sees only the method | built | `#[injectable] impl`, `impl_form.rs` | — |
| O1-new | `new` picked by default; an explicit mark wins | built | `impl_form.rs:92` | — |
| O1-fallible | Fallible constructors (`Result<Self, E>`) | built | DESIGN §3.4, §4 | — |
| O1-optional | `Option<Inject<Cache>>` works unchanged | built | `Option<S>`, DESIGN §3.2 | The rebuild's precise rule: `None` exactly when `S` would fail with `NotFound`; every other error propagates. |
| O1-async | Async constructors are "a real fork in the design" | built | `Construct::construct` is async; DESIGN §4 | — |
| O1-closures | Third-party types through closures whose parameters are injections, axum-style arity impls; no `Container` in a user signature | built | `Factory<Args>` per arity, `binding/factory.rs`; no container type in a user-facing signature | — |
| O1-both | Both placements on one type is a compile error through conflicting impls, clarified by a diagnostic or a doc | UNVERIFIED | Relies on E0119 between the two forms' `Construct` impls; the message is rustc's own unless a doc says otherwise | Needs a compile to see the message. |
| O1-construct | `#[construct]`, not `#[ctor]`/`#[constructor]` | built | `lib.rs:32-37` | F288 row. |
| O2 | `Dep<T>` over `Inject<T>`: types are nouns (axum's `State`, actix's `Data`, Bevy's `Res`); `Inject<T>` beside `#[inject]` gives one word two meanings | built | `Dep<T>`; DESIGN §0.3 makes "types are nouns" a principle | — |
| O3-conv | Attribute conventions: an attribute describes its item (`#[injectable]`), a role attribute marks a role (`#[construct]`), a helper is namespaced under the crate or the main macro (`#[serde(..)]`) | built | `#[injectable(default)]` | The verb argument: `#[inject(default)]` reads "inject the default" and means "do not inject; use `Default`". |
| O3-dyn | Trait objects in `Dep<dyn T>`; push `Send + Sync` onto the trait; the trait must be dyn-compatible, and native `async fn` in traits is not | built | DESIGN §5; role traits use `impl Future` with dyn-compatible erased twins as role keys, §3.7 | — |
| O3-types | Modules as distinct types: dedup, errors naming modules, configurable modules | built | DESIGN §3.6, §8.3 | — |
| O3-pushback | User: modules can and should be distinct types; `#[inject(default)]` blur is moot once `default` retires; the double wrap on a pool does not matter under the modelled core | built | Modules are types; the double wrap is accepted (DESIGN §3.2 `Dep::from_arc`; the core model's "cost accepted: a handle already an `Arc` inside sits behind one more pointer") | — |
| O4 | `Dep` naming kept; type-level lists for compile-time checking as a later wave | dropped (declined) | G.6 | — |
| O5-table | Visibility as a table the builder computes, one blanket impl | built | V-1 | — |
| O5-id | `ModuleId { TypeId, name: type_name }`; `type_name` is unstable in format but meant for diagnostics | built | `TypeName { id, name }`, DESIGN §3.1 | — |
| O5-all | Report every ambiguity in one pass | built | V-b | — |
| O5-local | A local provider beside an imported export is an error, with an explicit override | built (partly) | Refused (V-a); no override on the declaration; a test override exists | — |
| O5-trans | Only direct imports' exports are visible; re-export explicit | built | DESIGN §8.2, §8.1 | — |
| O5-owner | Singleton cells in the owner; dedup by `TypeId` guarantees one owner | built | DESIGN §3.6 | — |
| O5-cycles | Import cycles get the same DFS check as dependency cycles | built | V-f: open as a question | — |
| O5-use_from | `m.use_from::<dyn UserRepo, PersistenceModule>()` in the help line | dropped | V-c | — |
| O5-ct | Compile-time index trick; E0283 cannot name modules; `on_unimplemented` does not fire for ambiguity | dropped (declined) | G.6 | — |

---

## 8. Worth discussing with the designer

Each is a question with a recommendation.

1. **Should module import cycles be tolerated again?** (V-f.) The rebuild refuses them; master
   tolerated and pinned them; the visibility review rejected refusing them on the ground that an
   import edge is a visibility fact, and the rebuild's collection order already skips a closing edge.
   *Recommend:* tolerate an import cycle with no provider cycle in it, keep the provider-cycle
   diagnostic as the refusal, and port master's `import_cycle.rs` shape as the test. If the refusal
   stays, record the reason in DESIGN §10.1 beside step 1, so `SWITCH_INVENTORY.md` Q3 has an answer.
2. **Should identity cover every setting of a configured module, or should a differing duplicate be
   refused?** (F152 at `StoreModule`.) *Recommend:* refuse two imports with one identity whose
   `Module` values differ, naming both import sites. Folding every knob into the identity would build
   two pools for one URL where a user meant one, and the refusal is cheaper to explain. The rule
   belongs in core: `Registrar::enter` can compare the second module with the first where both are
   `Eq`, or each configured module can expose its full settings as the identity's value.
3. **Should a contribution be able to name an existing binding?** (F256.) *Recommend:* yes, an
   `into` entry `alias T` lowering to a `Contribute` method that reads the single binding `T` and
   coerces through the closure the macro writes, so one instance serves both the direct read and the
   collection.
4. **Should a factory whose output implements a hook trait be refused, or pointed at `provide_with`?**
   (F139's class.) *Recommend:* a compile-time probe at the `ModuleDef` factory methods and in
   `#[module]`'s lowering, emitting a warning-grade refusal naming `provide_with`. If a compile-time
   probe cannot reach the value API's generic methods (Halt 3's limit applies to a generic function),
   confine it to the macro, which writes the concrete call site, and document the value API rule as it
   stands.
5. **Should `Dep`, `Many` and `Ext` implement `Debug` writing the key?** (G.2.1, P6.) *Recommend:*
   yes, as the core model specified. It costs three impls and lets every injectable derive `Debug`
   without printing its dependencies' state.
6. **Should the ambiguity report name a module's own binding as bound, not exported, and carry a
   remedy?** (V-a.) *Recommend:* yes, "bound by M" for the module's own source, and a `help:` naming
   the three fixes; plus two tests pinning own-beside-import and import-beside-global as `Ambiguous`.
7. **Should a missing `T` name a bound `Result<T, _>`?** (D9.) *Recommend:* yes, as a near-spelling
   case in `near_spelling`, with the hint "the factory returns a `Result`; register it with
   `try_singleton` (or write `?` in `#[module]`)". The value API's binding of a `Result` is
   documented and stays.
8. **Should the connect order be depth-first over declarations, as D3 specified?** (D3.) A judgment
   call; both orders are stable and topological. *Recommend:* keep the rebuild's order and state in
   DESIGN §9.2 that ties break by id, so a module declared out of order runs its independent bindings
   first. Record the module-hook position (after its providers') as a decision, since the core model
   said the opposite.
9. **Port two verification tests the handoff specified.** Two global guards in one module and across
   two modules both run, in collection order; adding an unrelated binding changes no hook order
   (tested with at least two names). *Recommend:* add both to the core suite.

---

## 9. Not verifiable here

| Item | Needs |
| --- | --- |
| O1-both / P2c: the message a user gets for `#[injectable]` on both the struct and the impl of one type | A compile of the two forms on one type; the claim is E0119 on the generated `Construct` impl |
| F152 at `StoreModule`: which instance's settings apply when two imports differ only in a knob | A run with two imports, one `.lazy()`, observing whether the pool connects at `connect`; reading says the first reached in the registration walk (`register.rs:145-151`) |
| The probes P1-P8, Pv1-Pv5, P2d and the `ulo-probe` binaries | Their sources are gone from the scratchpad; each claim above is checked by reading the rebuild, and the language facts (P1a, P1c, P1d, P4b-P4d, P5, P7, Halt 4's `Unsize`, Halt 8's E0119/E0117) would need re-probing to restate as Probed |
| F43, F16, F261, F266, F278, R41 (F226) | Other subjects' readers (naming, gRPC, WebSocket, pipeline, RPC) |
| `behavioral-traits-audit` (parked thread) | The memory note's content; no artifact in this folder states what it proposed |
