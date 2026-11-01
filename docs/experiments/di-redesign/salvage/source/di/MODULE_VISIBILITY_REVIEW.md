# Module visibility — review of the outside proposal

Written 2026-10-01 against the worktree `toni-rs-r33` at `95892262`, the top of the open stack.
Scope: the outside chat's proposal that a module's visibility be "a table the builder computes"
under one blanket `Resolve<T>` impl, with its seven policy points, put against `DI_CORE_MODEL.md`
and the tree. The memo stands alone for a reader holding the model document and the proposal.

**Where the findings live.** `FRAMEWORK_GAPS.md`: F290 rewritten (the tier order is a decision
the model document mis-stated, and the question is put below), F291 (a re-export is accepted at
scan and stalls `create`), F292 (one refusal per `create`).

**Not a build.** No branch exists and nothing in `toni-rs-r33` was edited. `DI_CORE_MODEL.md` was
corrected where this review falsified it; the edits are listed at the end.

**Confidence marks.** *Probed*: run this session against `95892262`; the appendix lists the probes.
*Read*: read off the source, cited by file and line at `95892262`. *Asserted*: reasoning from the
two, not run.

**The shape of it.** Four independent questions. Whether the proposal's mechanism differs from the
loader's (it does not; §1). Whether the tier order the loader resolves in is a defect or a rule
(§2a, the one decision with weight). Whether refusals should be collected before construction
rather than raised during it (§2b, a mechanism the loader lacks). Whether a `where R: Resolve<T>`
bound checks anything (§3, settled by two probes).

## 1. What the proposal changes in the model

**Already built, under other names** (Read). "One blanket impl, visibility as data" is the loader.
`resolve_dependencies` (`crates/ulo/src/di/internal/instance_loader.rs:621-714`) answers each
dependency key from the module's own instances, then a pre-registered instance, then its direct
imports' exports, then the global registry, then a collection; `resolve_from_imported_modules`
(`:716-767`) collects every import exporting the key and refuses two naming both. The table is
computed per dependency during construction rather than once before it, which is the one
structural difference (§2b). Singleton cells live in the owner: an importer reads the owner's
instance map (`:751-759`), and dedup on the `ModuleIdentity` key in the scanner (`scanner.rs:32-38`)
guarantees one owner. `ModuleIdentity::of_type` already carries `type_name`
(`di/module/identity.rs:21`). Only direct imports' exports are visible (Probed Pv5a).

**Matches the earlier probes** (Read against `fable-inject/std/p2a`, `p4a`–`p4d`). The proposal's
diagnosis of the overlap — one impl per import, no negative bounds, E0119 — is P4b. Its
compile-time alternative — an index parameter inferred when exactly one applies, E0283 on
ambiguity, `on_unimplemented` not firing for it — is P4a and P4c, the shape §6 of the model records
under F287. Its recommendation of the runtime shape is the model's.

**New.** (i) The `macro_rules!` callback for naming both modules at compile time: each module's
macro emits a callback listing its exports and the importer's macro emits `compile_error!`. It
names modules only when both are `#[module]` structs, a module built by a function having no macro
to emit one, and compares exports as token text, the spelling problem ADR-0059's type keys removed
(Asserted: `macro_rules!` matches token trees and cannot compare two `ty` fragments for equality).
Recorded in §6 of the model as a limit of F287, not a route. (ii) Refusing a local binding beside
an imported export: §2a. (iii) Every ambiguity in one pass: §2b. None of the three touches
`Dep<T>`, the site trait or the value API.

**Two differences the proposal carries without saying so.** Its table refuses an ambiguous key
whether or not anything injects it; ulo refuses only where a provider or controller injects it, and
a module importing two exporters of one key starts when nothing there asks for it (Read:
`integration-tests/tests/integration/ambiguous_lookups.rs:83-98`). That is why `use_from` is
load-bearing in the proposal and optional here (§2c). And its resolver is pulled by the
constructor, `r.resolve()` per field with the key read at construction, where ulo's factory
declares its keys ahead of building (`ProviderFactory::dependency_tokens`, read at
`instance_loader.rs:357`) and is handed the answers; the declared list is what makes a
pre-construction check possible (§2b).

## 2. The policy points

### (a) A local binding beside an import's export, and the globals

*ulo now* (Probed Pv1–Pv3): a module providing `Region` and importing a module exporting `Region`
injects its own, and `.visible()` answers its own; the same beside a global module exporting
`Region`; an import's export beside a global's injects the import's. Nothing is reported in any of
the three. The mechanism is the step order at `instance_loader.rs:631-672` and the local-first
match at `module_ref.rs:206-227` (Read). Import-over-global is pinned as intended by
`visible_reads_an_import_before_the_global_registry` (`ambiguous_lookups.rs`, "an import's export
ahead of a global module's"; Read); local-over-import and local-over-global are pinned by nothing.

The tree implements lexical shadowing across tiers and refusal within one. Two imports have no
order between them, and the question is refused; a local binding, an import and a global have one,
and the nearer answers. ADR-0057 states the three tiers as an order and the test's doc comment
states it as intended (Read). F290 as filed read a decided order as a defect. What is undecided is
whether the order is the rule the user wants, and the model document mis-stated it: §0 sentence 4
and §1.3 said "refused, never guessed" with no tier qualification. Both are corrected.

*The proposal's argument* is that shadowing is how a test mock leaks into production wiring. In ulo
the leak needs the mock declared in the production module's own `providers:` list: a test root's
binding does not reach into a module it imports, visibility being per module (Read, the step
order). The override a test wants — replacing one binding of a module it does not edit — does not
exist in ulo with or without shadowing. That is Nest's `overrideProvider`, a testing API, and out
of scope here.

*Verdict:* the user's, D1. Recommend keeping the tier order, with the model stating it. The
alternative costs an override declaration that does not exist and whose module-naming half waits
on F286, flips the pinned import-over-global test, and needs a sweep of the suites for sites that
shadow today, unmeasured.

### (b) Every ambiguity in one pass

*ulo now* (Probed Pv4): `create` names one refusal. Two modules each with an ambiguous key: one
module's. One module with two ambiguous keys: one key's. A missing key in one module and an
ambiguity in another: the miss. Mechanism: `create_instances_of_providers` applies `?` to each
provider's resolution (`instance_loader.rs:359`) and the phase-1 loop returns on the first
`LoadError::Failed` (`:160-162`); only deferrals are collected, for the stall diagnostic
(`:166-172`) (Read). ADR-0057's refusal is `resolve_from_imported_modules`, which says
"exporters", which is why a grep for `Ambiguous` finds nothing in the loader; `AmbiguousModule` is
the runtime lookup's variant, raised at `module_ref.rs:221`.

*Verdict:* adopt, as F292. The inputs exist when the scan ends: every provider's and controller's
dependency keys (`dependency_tokens`), every module's declared exports (`exported_tokens`), the
global token set (filled at `scanner.rs:237`) and the import edges. A check beside
`refuse_a_key_bound_both_ways` (`scanner.rs:84`) can compute, for each key a module's provider or
controller injects, its set of sources, and report every miss and every ambiguity in one error
before any provider is built — so no `for_root` pool connects ahead of a later module's refusal.
The loader's per-dependency resolution stays as the runtime table. Cost: one walk over declarations
and a message format for a multi-item refusal.

### (c) An explicit pick, `use_from::<K, M>()`

*ulo now* (Read): none. The refusal names two fixes, import one of the two or bind one under a
marker (`instance_loader.rs:741-744`). Under consumer-gated refusal a module importing two
exporters of `Db` for their other exports starts.

*Verdict:* defer. Under the proposal's eager refusal every such import pair needs `use_from`; under
ulo's the case is a module that injects `Db` and needs both imports for other keys, which the marker
covers at the cost of that module's sites spelling it. `use_from` names a module, and a module is a
string until F286; a pick keyed by type belongs in F286's row if the marker cost proves real.

### (d) Transitive exports and re-export

*ulo now* (Probed Pv5): only direct imports' exports are visible. `Top` importing `Mid` importing
`Base` cannot inject `Base`'s export ("Dependency not found"), the proposal's rule. A re-export,
`Mid` listing `X` in `exports:` and importing the module that provides it, is accepted at scan and
fails at `create` as a stall: "imported module `Mid` exports `X`, whose instance is not created
yet". It never will be: a module's built-export set holds only what it built itself
(`add_export_instances_tokens`, `instance_loader.rs:480-493`, Read). Nest's spelling for the same
intent, a module type in `exports:`, is accepted by the macro (`exports: Vec<Type>`,
`ulo-macros/src/module_macro/module_struct.rs:18`) and fails the same way (Asserted from the
mechanism; not run).

*Verdict:* adopt the proposal's re-export as F291, in one of two forms (D2). Either an `exports:`
entry a direct import exports into the module is forwarded, and one nothing provides or forwards is
refused at scan naming the module and the key; or every unprovided export is refused at scan.
Recommend the first: it is the transitive mechanism the proposal wants, and the refusal half closes
the stall either way.

### (e) Singleton cells in the owner, and dedup by `TypeId`

*ulo now* (Read): an importer reads the owner's instance (`instance_loader.rs:751-759`); dedup is
on the `ModuleIdentity` key, base plus config fingerprint, in the scanner (`scanner.rs:32-38`). A
`#[module]` struct's base is `type_name` (`of_type`); a `DynamicModule`'s is the builder string
with the providers' `identity_hint`s hashed in (`derive_identity`, `di/module/dynamic.rs`); a keyed
connection's is the marker's `type_name` (`postgres_keyed::<K>`, `ulo-db-sqlx/src/module.rs:66-84`).

`ModuleId { TypeId, name }` loses the fingerprint. Every `DynamicModule` and every `CheckedModule`
has one `TypeId`, so every `for_root` of every integration would be one module, the shape PR #126
fixed: two `for_root` URLs, one connection, nothing reported. After F286 gives each `for_root` a
nominal type, `TypeId` tells `SqlxPostgres` from `SeaOrm` and still not `SqlxPostgres { url: a }`
from `{ url: b }`, and the fingerprint stays, as F286's entry says. What `TypeId` adds over
`type_name` is one distinction the string collapses: two versions of one crate in a graph give two
types with one `type_name` (Asserted). A keyed constructor returning `SqlxPostgres<K>` is distinct
by `TypeId` alone. *Verdict:* nothing to adopt; one line added to F286 in the model for `TypeId`
beside the name.

### (f) Module import cycles

*ulo now* (Read, pinned): a mutual import boots both modules and a provider resolves across the
cycle (`integration-tests/tests/integration/import_cycle.rs:49`, `:105`); `ordered_module_tokens`
appends the cyclic remainder in sorted order (`container.rs:528-547`) and the phase-1 retry orders
construction. An import edge is a visibility fact, not a construction edge, and the construction
cycle underneath, when there is one, is named by the provider-level diagnostic.

The proposal gives the import cycle "the same DFS check as dependency cycles" and no argument past
that parity. Under its own model the table computes over a cyclic graph unchanged: visibility is a
lookup, and only construction order needs the DFS, which the provider graph has. Refusing the
import cycle refuses `cross_module_dependency_through_import_cycle_resolves`, a shape with no
provider cycle in it. *Verdict:* reject; nothing in the proposal changes the memory note's reason.

### (g) `Module::register(&self, &mut ModuleBuilder)`

*ulo now* (Read): `#[module(imports, providers, controllers, exports, global)]` on a struct or an
impl, generating the four list methods of `ModuleMetadata` (`module_struct.rs:351-361`);
`DynamicModule::builder(id).provider(..).controller(..).export::<T>().global().build()` for a module
built by a function; a hand-written `impl ModuleMetadata` with `&self` where a module carries state
(`CheckedModule`, `BroadcastModule`). `imports:` takes expressions, so `imports:
[DbModule::for_root(url), AuthModule]` mixes values and paths; `m.bind::<dyn UserRepo, PgUserRepo>()`
is `provide!(dyn UserRepo => PgUserRepo)`.

The proposal's shape is `ModuleMetadata` with one builder in place of four list methods, and its
`&self` is what lets a config-carrying module read its fields, the F286 shape. *Verdict:* defer to
F286's row, where the question is whether a nominal `for_root` type implements `ModuleMetadata` by
hand, through a builder method, or through `#[module]` on a non-unit struct. The declarative
attribute stays the promoted spelling either way.

## 3. The `Resolve<T>` / `Construct<R>` bound

*Probed* (P2a, `fable-inject/std/p2a_resolve_vacuous.rs`): over a container with one blanket
`impl<T> Resolve<T>`, `where R: Resolve<Db> + Resolve<Uow>` holds for every `T` and construction
compiles against an empty container. The proposal's `ModuleResolver` is that container, and the
bound checks nothing about the graph. *Probed* (P2d, `fable-inject/std/p2d_fake_resolver.rs`): the
one place it has teeth is a hand-written resolver implementing `Resolve<Db>` and not
`Resolve<Uow>`, refused E0277 naming the missing one.

That is a real use, a test double for construction outside the container, and the `Dep<T>` model
gives it without a trait: the constructor is an ordinary function, `UserService::new(Dep::from(
Arc::new(FakeDb)), ..)`, and a missing dependency is a missing argument (Asserted). The generic
`Construct<R>` also gives up the declared dependency list §2b rests on. *Verdict:* decorative for the
graph, redundant for tests; the model's `Site` trait and `ProviderFactory::dependency_tokens` stand.

## 4. Decisions that are the user's

**D1 — Is the tier order a rule or a defect?** (i) Keep lexical shadowing across tiers and refusal
within one; the model states it, the two unpinned pairs get a test, F290 closes as decided. (ii)
Refuse across tiers too, with an override on the local declaration (a modifier on the `provide!`
value, ADR-0058's natural place) and on the import for import-versus-global; the pinned
import-over-global test flips; a sweep measures what shadows today. *Recommend (i).* The proposal's
leak needs the production module edited, and the override a test wants is a testing API, not a
visibility rule.

**D2 — Re-export.** (i) Forward an `exports:` entry a direct import exports into the module, and
refuse at scan one nothing provides or forwards. (ii) Refuse every unprovided export at scan.
*Recommend (i)*, F291.

**D3 — One-pass reporting before construction.** Adopt F292 as a small row now, or leave it to the
F287 wave. *Recommend now*: a scan-time walk over lists the scanner already holds, and it stops a
pool connecting ahead of a later refusal.

## 5. Out of scope

A testing-module override API (Nest's `overrideProvider`): the proposal's leak argument points at
it, and it is neither visibility nor this review's. F281, collections gathering regardless of
exports, is untouched: F292's scan-time table reads single bindings only. F287's compile-time
layer: the proposal's `macro_rules!` route is recorded in the model as a limit.

## Edits made to `DI_CORE_MODEL.md`

§0 sentence 4 and §1.3 *Visibility*: the tier order stated as precedence, with the direct-import
limit, the re-export stall and the one-refusal limit (Pv1–Pv5). §1.3 *Identity*: `TypeId` beside
the name under F286. §6: the `macro_rules!` route as a limit of F287. Appendix: the six probes
below.

## Appendix: probes

Under `/private/tmp/claude-501/-Users-ifeanyi-Projects-Personal-open-source-toni-rs-toni-rs/a632688a-d566-4ff7-b727-8f28e1dd90c6/scratchpad/`,
against `toni-rs-r33` at `95892262`, rustc 1.98.1, edition 2024:

| Probe | Shows |
| --- | --- |
| Pv1 `fable-visibility/src/bin/pv1_local_beside_import.rs` | a local binding beside an import's export: injection and `.visible()` both answer the local |
| Pv2 `fable-visibility/src/bin/pv2_local_beside_global.rs` | the same beside a global module's export |
| Pv3 `fable-visibility/src/bin/pv3_import_beside_global.rs` | an import's export beside a global's: the import is injected |
| Pv4 `fable-visibility/src/bin/pv4_one_pass.rs` | one refusal per `create`: one module's, one key's, a miss ahead of an ambiguity |
| Pv5 `fable-visibility/src/bin/pv5_reexport.rs` | a transitive export is not visible; a re-export stalls `create` with "not created yet" |
| P2d `fable-inject/std/p2d_fake_resolver.rs` | a partial fake resolver is E0277 under the `Construct<R>` bound |
