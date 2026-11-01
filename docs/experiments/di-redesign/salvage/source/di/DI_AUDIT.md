# Audit — the DI system and the core fabric

The pass `DI_AUDIT_HANDOFF.md` asked for: a full re-walk of the DI system and the core fabric,
*"its structure, cleanliness, dx, redundancy, polishness, betterness and improvement."* Run
2026-09-21 against master `654c7183`.

## Where the findings live

`FRAMEWORK_GAPS.md`, **F138–F162**. This memo does not restate them; it says what they add up to and
which calls are the user's. **F37 is cleared** — the `DEFERRED:` string sentinel it describes is gone,
replaced by a `LoadError::Deferred` variant the loader matches by type
(`crates/ulo/src/di/internal/instance_loader.rs:18-23`), and the literal appears nowhere in the
workspace.

## Not a build

No branch exists and nothing was committed. The repository is clean at `654c7183`. The only files
written are this memo, the ledger entries, and `DI_BUILD_HANDOFF.md`.

## Confidence marking

- **Probed** — run this session against `654c7183`, in a scratch crate sharing the workspace target
  directory, on rustc 1.98.1 (the version `dtolnay/rust-toolchain@stable` installs in CI). Every
  compile probe ran beside a control that must pass and a control that must fail; every runtime probe
  ran beside the same shape in its working form. Two probes measured the author's own mistake before
  the control caught it: one read a `Body::text("x".into())` ambiguity as a framework refusal, and one
  concluded `provide!`'s second token classifier was reachable when the round-trip through
  `token_type_to_tokens` had already discarded it. A third, on the second pass: a two-entry-point
  factory design passed its own probe and was falsified by putting the async form through the sync
  door, which compiled a provider of futures rather than refusing.
- **Measured** — counted or diffed by a script that was self-tested first. §9's four-resolver
  comparison is the case: a first extractor returned two-line function bodies, which would have read
  as "these are nothing alike"; re-run with an assertion that every body exceeds its signature, the
  same comparison found two of the four identical in 153 of 163 lines. The measurement then **refused
  the fix it was run to support** — the controller's resolver is 95% different and is not a copy — so
  the recommendation changed from unifying four functions to sharing one branch.
- **Read** — read off the source, cited by file and line.
- **Unverified** — believed and not established. Marked inline where it appears.

No claim here is Spec: nothing in this pass is a conformance question. Dependency injection, modules,
scopes and tokens are category (3) under `project-the-spec-conformance-seam` — no published
specification mentions them, and *better* means the design is coherent and one idea is not spelled
three ways.

## The shape of it

Nine independent problems. They can be taken in any order except that §4, §5 and §9 share a fix, and
§1 overlaps the pipeline build's PR 5.

Three of them are one mechanism seen three times: **the container answers an ambiguous question by
taking whichever entry a hash map hands it first, and reports nothing**. A duplicate token (§1), the
order lifecycle hooks fire in (§2), and which module answers a token two modules hold (§3) are the
same defect at three sites. Two more are one mechanism seen twice: **a macro decides what an
identifier means by looking at how it is spelled** — the casing rule for a token (§4) and the name of
a constructor (§9.3). A third pair is one idea spelled twice and then copied: the declaration surface
carries two vocabularies over ten code paths (§9.1), and the module declaration grammar carries four
(§5). The rest are separate: two diagnostics (§7), and the surface's redundancy (§8).

§9 was added on the second pass, 2026-09-22. §1–§8 are the first.

---

## 1. A token collision is last-wins, everywhere, with nothing said — F138

**Probed.** Two providers under one token: the second replaces the first and `create` succeeds. Where
the framework fixes the token the cost is worse: two `provider_token!(APP_GUARD, …)` registrations in
one module run the surviving guard twice and the other never, and the same pair split across two
modules does the same. Registering a global guard per module is the documented Nest shape, and here
the second registration removes the first.

The framework already refuses one instance of this — ADR-0029's global-export clash, which names both
modules and points at `for_root_named`. That refusal covers exactly the case where two *global*
modules export one token. Everything else is unguarded: two providers in one module, two imports
exporting one token (§3), and every `APP_*` registration after the first.

**This overlaps the pipeline build.** Its PR 5 replaces the global registration surface with "one
surface per transport per role, taking a token or a value" (`PIPELINE_BUILD_HANDOFF.md`). A
token-taking surface can collide the same way. PR 5 has to say what two registrations mean, and the
pipeline audit did not raise the question. See the decision in D2.

## 2. Lifecycle hooks fire in hash order — F140

**Probed.** `on_module_init` and `on_application_bootstrap` iterate `Container::module_tokens()`,
which is the container's `FxHashMap` key order, while the loader one phase earlier uses
`ordered_module_tokens()`. Inside a module the providers come from another map. A provider's hook can
run before the hook of a provider it injects, and adding one unrelated provider to a module flips two
existing hooks — reproduced with four different added names.

The three shutdown phases read the same map and walk it forwards, so teardown is not the reverse of
construction. A provider closing a pool and a provider flushing to it are ordered by accident.

Construction order is correct: the per-module `DependencyGraph` topologically sorts providers, and
`ordered_module_tokens` sorts modules. The ordering exists one layer below the hooks and is not
reused.

## 3. Which module answers is arbitrary, and `.global()` is not the global registry — F141, F142

**Probed.** Two defects at the same seam.

`ModuleRef::…global()` searches every provider of every module, exported or not. A provider in a
module that exports nothing and is not global is reachable from a module that does not import it.
Module encapsulation has an opt-out, and the method that opens it is named after a different
framework concept — the global registry `#[module(global: true)]` fills.

Separately, a token held by two modules resolves to one of them at three sites: an import set
iteration in the loader, `ModuleRef`'s non-strict branch, and the application context's by-type
lookup. Renaming one module's builder id, with declaration order unchanged, flips which value an
injection site receives.

## 4. The token language — F143, F148, and F50

This is the thread the handoff named as most likely to pay, and the probe matrix is what it looks
like. **Probed**, each row against a passing control:

| Spelling | `#[inject(..)]` | `provider_token!` | `provide!` | `#[module(exports:)]` | `#[use_guards]` | `get_by_token` |
| --- | --- | --- | --- | --- | --- | --- |
| `"STRING"` | yes | yes | yes | **no** | yes | yes |
| `Type` | yes | yes | yes | yes | yes | — |
| `path::Type` | yes | yes | yes | **no** | **no** | — |
| `CONST` (bare) | yes | yes | yes | **no** | **no** (F50) | yes |
| `path::CONST` | **no** | **no** | **no** | **no** | **no** | yes |
| `Type` spelled all-caps | yes | yes | **no** | yes | yes | — |

The cause is one function: `is_const_identifier` decides type-or-const by asking whether the path has
one segment and is written in capitals. A `Token<T>` const in a `tokens` module — where a project puts
them — is unusable in every macro position. A type named `HTTP` is read as a const. The refusals are
`E0573`, `E0423` and `E0747`, none of which names the constraint, and `#[use_guards]`'s own message
lists four supported forms without mentioning that the path must be one segment.

A second classifier exists and decides nothing: `provide!` classifies its token with a looser rule,
renders it back to a bare path, and hands it to a handler macro that re-parses with the stricter one.

The fix is the one the spelling audit always reaches: stop reading the spelling. Accept any path in
token position, emit the `IntoToken` call, and let the compiler reject a path that is neither a
`Token<T>` nor a `&str`. That removes the casing heuristic, the `__UloTokenName` local trait, the
second classifier, and F50, in one change.

## 5. The module declaration grammar — F144

**Probed.** `#[module]`'s four list keys parse four grammars. `imports:` and `providers:` take an
expression; `controllers:` and `exports:` take a bare identifier. A module cannot export a
string-token provider it declared with `provide!`, a provider whose type is generic, or a provider
from a submodule, and cannot declare a path-qualified controller. `DynamicModule`'s builder has both
export forms and accepts what the attribute refuses. The divergence runs the other way as well:
`Extension<T>` is declarable in `providers:` and not through `builder().provider::<T>()`, the two
surfaces reading two different declarations of the same fact.

`exports:` is the sharpest: the documented way to register an extension view
(`providers: [Extension::<CurrentUser>]`) produces a provider no module can export.

## 6. Lifecycle hooks a wrapper drops, and one triggered by a name — F139, F145

**Probed.** `provider_token!` forwards `token`, `scope` and `resolve`, and the five lifecycle hooks
default to no-ops, so a type with `#[on_module_init]` registered under a custom token never fires it.
The control is the same type registered plainly in the same module, which does.

This joins three already filed. A8: `provider_value!`/`provider_factory!`'s `lifecycle` flag emits
code that does not compile. A9: a hook on an execution- or transient-scoped provider compiles and is
never called, with the rejection unreachable. C17: a per-call dispatch target discards its init hook's
`Err` and never fires its shutdown hooks. Four separate holes in one contract, each found separately.
The contract itself is stated nowhere: no doc says which registration forms preserve hooks.

Separately, `init = "from_request"` is a magic string. The macro branches on the init method being
named that literal and prepends an HTTP request argument, against a generated body that no longer
binds the variable it references. Renaming a constructor from `build_it` to `from_request`, nothing
else changed, turns a compiling provider into `E0425: cannot find value __http_ctx in this scope`. The
form is retired — its only occurrence in the workspace is a doc comment calling it "the old magic".

## 7. Two diagnostics — F146, F149

**Probed.** A provider cycle inside one module reports `Circular dependency detected for provider:
di_probe::CB` — one token, no chain, no remedy. The same cycle across two modules reports the full
chain and two remedies. The better message is produced by `find_dependency_cycle`, which sits in the
same file as the worse one and is called only from the stall path.

**Read.** The Phase-1 retry budget (`max_iterations = total_modules * 2`) cannot be exhausted: every
surviving pass shortens the pending list by at least one, and a pass that shortens it by none returns
the stall diagnostic. The counter and the `Module instantiation timed out` error below it are
unreachable, and the message names a timeout where there is no clock.

## 8. Surface and redundancy — F147, F150, F151, F152, and two already filed

**Probed.** `UloApplication::new` is `pub`, takes `Arc<RwLock<Container>>`, and cannot be called from
outside the crate. It is also what keeps `Container` counted as reachable by `#![deny(unreachable_pub)]`
— a `pub` item added anywhere under `di/internal` fails the build, and a `pub(crate) use` re-export
does not suppress that, so the signature is the loophole. F44's DI half is otherwise closed by the
#278-#290 reorganisation and this gate; this is the one item left.

**Read.** The rest:

- `Arc<Box<dyn Provider>>` runs through the SPI, the store, the container's maps and every generated
  factory — 26 occurrences in core, 62 workspace-wide. `Arc<dyn Provider>` is the same value with one
  hop. Whether the hop is measurable is the performance pass's; the redundancy is not.
- A startup check is outside a module's identity, so two imports of one URL with different checks
  dedup to whichever the scanner reaches first.
- Four in-source documentation defects, including an orphaned doc block that the recorded gotcha
  *a doc block attaches to whatever follows it* predicts exactly, and DOCS_AUDIT B23's text still in
  place at its new path.
- Already filed and unchanged: **F45** (three `TypeId`-keyed maps written three times) and **F73**
  (four files under `di/` serving one transport, `MiddlewareConsumer` the substance). F73 is the
  pipeline build's PR 12.

### Checked and sound

Recorded so they are not re-proposed.

- `di::Execution` beside `ProviderScope::Execution` is deliberate, argued in `5548518c`'s body under
  ADR-0047. Not a collision.
- `ModuleMetadata`'s declarative accessors look like the shape
  `project-behavioral-traits-audit` targets, and the laziness exception applies and is load-bearing:
  `imports()` is a method body evaluated at scan time, which is why an import edge records no
  construction dependency and module import cycles are free. Collapsing it to a descriptor would
  forfeit that.
- Two modules each declaring the same provider type build two instances and are accepted. That is
  module-scoped providers working, not a collision.
- Module construction order is correct at both levels. The defect in §2 is that the hooks do not use
  it.

## 9. The declaration surface, and how a provider is constructed — F153–F162

Added 2026-09-22 after the first pass under-covered it. The handoff's scope named the provider SPI,
`DynamicModule` and multi-providers; §4 audited how a token is *spelled* and left how a provider is
*declared*. This is that.

### 9.1 Two vocabularies over ten code paths — F161

Five acts, spelled twice. `provide!` with five markers, 48 call sites; `provider_value!`,
`provider_factory!`, `provider_alias!` and `provider_token!`, 100 call sites between them. They are
not alternatives — `handle_provide` classifies its input, renders it back to tokens and hands it to
the other four, emitting no provider impl of its own.

Underneath, `multi` is a second copy of every kind rather than a property of a binding:
`generate_type_multi`, `generate_value_multi`, `generate_factory_multi`, `generate_alias_multi`,
`generate_token_provider_multi`. Five acts by two channels is ten code paths in 1996 lines.

AUDIT3 §4's rule settles what that means: `multi` is not a kind. Prototyped and compiled, `under`
and `multi` are one blanket extension over `ProviderFactory`, composing over every constructor and
over a type's own declaration, and chaining.

### 9.2 What the surface becomes

Prototyped against the real SPI, every line below compiled.

```rust
providers: [
    Db,                                               // the type declares itself
    Provide::value("API_KEY", key),
    Provide::factory("POOL", async |c: Config| open(c).await),
    Provide::alias("LOG", "LOGGER"),
    FileLogger::provide().under("LOGGER"),            // was provider_token!
    A::provide().multi::<dyn Plugin>("PLUGINS"),
    B::provide().multi::<dyn Plugin>("PLUGINS"),
]
```

Three constructors, one accessor, two blanket modifiers. Five proc macros are deleted.

**Why a value and not a macro.** One thing a macro bought: `provider_factory!` reads the closure's
annotated parameter types to emit `dependency_tokens()`. A trait implemented per arity supplies the
same tokens from the *types* rather than from the text, which is the fix shape the spelling audit
reaches every time. With that, nothing else in the surface needs a macro.

**Why the token stops being a token tree.** A token in value position is an `impl IntoToken`, so
every spelling §4's matrix refuses works: a qualified const, a bare const, a runtime `String`. That
is D4's outcome reached by deleting the parser rather than by fixing it. A typed token also becomes
load-bearing: `Provide::value(tokens::API_KEY, 42u32)` where `API_KEY: Token<String>` fails with
`expected String, found u32`, while the string form accepts it.

**Why a factory is always async.** Recorded as D9.

### 9.3 Construction — F156, F157, F158, F159

Four spellings, with `DependencySource`'s four "sources" as a fifth vocabulary over the top.

The two constructor mechanisms take their dependencies from different places. `#[new]` reads the
method's **parameters**; `init = "…"` reads the struct's **`#[inject]` fields** and passes them
positionally. Probed: the same struct moved between the two forms resolves different dependencies,
and the diagnostic points at the constructor rather than at the missing attribute.

`#[default(expr)]` is inert whenever `#[new]` is present — the constructor wins and the default is
never read, with no warning.

`#[controller]` has already chosen. It refuses `init = "…"` and names `#[new]`, in a message whose
closing clause — *"as with `#[injectable]`"* — is false today. Removing the key from `#[injectable]`
is what makes that sentence true.

### 9.4 The naming — F160, F162

One word means one thing, and a type is named for what its consumer does with it.

| Concept | Today | Becomes |
| --- | --- | --- |
| Recipe: token, deps, build | `ProviderFactory` | unchanged |
| The thing that answers a resolution | `Provider` | unchanged |
| What `build` returns: instance + roles | `Injectable` | **`Registration`** |
| Marking a type a provider | `#[injectable]` | unchanged |
| Marking a dependency | `#[inject]` | unchanged |
| A type's own declaration | `provider_factory()` + `__ulo_provider_factory()` | **`T::provide()`**, one accessor |
| Bind under another token | `provider_token!` / `provider(T)` | `.under(token)` |
| Contribute to a collected token | `multi(Tr)` | `.multi::<Tr>(base)` |
| Widen a `ModuleRef` lookup | `.global()` | `.or_global()` |

`Registration` names what `add_provider_instance` does with it, its two fields being what to
register and as what. `T::provide()` makes the verb read the same in both positions —
`Provide::value(…)` and `FileLogger::provide()`. One consequence: `provide()` is a trait method, so
`DeclaresProvider` goes in the prelude, which is the same line that brings the modifiers.

Two further collisions the survey found: `existing`/`alias` and `provider`/`token` are four names for
two acts, and `ExtensionFactory<T>` sits 21 lines from `ExtensionsFactory` in one file, one public and
one private.

### 9.5 Three defects that leave `create` by panic — F153, F154

A dispatch target cannot inject a multi-provider collection: three of the four field resolvers handle
`Vec<Arc<dyn Trait>>` and the controller's does not, so the declaration compiles and panics at
startup. A factory depending on an execution-scoped provider panics the same way, the factory
resolver having hard-coded `Execution::None`. The generated execution-scoped provider panics on a
missing cache, which is the third.

AUDIT3 §4's measurement was run on the four resolvers and **refused the obvious fix**. Two of them
are one function written twice — 153 of 163 lines identical, differing only in where the dependency
map lives and which execution to resolve in (F155). The other two are 65% and 95% apart and are not
copies. So the controller's gap is one missing branch to extract and share, not a unification, and it
needs a change of its own: nothing else in the plan touches `controller_attr.rs`.

All three are the startup-failure policy unapplied. The constraint for the build: **no DI defect
leaves `create` by panic.**

---

## Decisions — all settled

Put to the user 2026-09-21 and 2026-09-22 and answered. **They are not open.** A recommendation kept
beside a decision is there as its argument, not as a choice still to make. The alternatives rejected
are recorded with each, because they are what gets re-proposed otherwise.

**D14 was settled after this audit closed**, on 2026-09-22, and belongs here rather than to the pass
that raised it: it is a statement about what a provider hands out. `EXTRACTION_AUDIT.md` §6 is where
it surfaced.

### D1 — what `.global()` means on `ModuleRef` — **narrowed, and renamed**

`.global()` becomes `.or_global()` and reaches only exported and global tokens.

Rejected: keeping the reach and renaming to `.anywhere()`, which was the recommendation. The user's
reasoning overrides it — a provider a module did not export should not participate, and a genuine
"search anywhere" need earns its own API when something asks for one. Nothing in the tree asks: the
single caller of the widening resolves a provider exported by a global module, so the narrowing
breaks no test.

`or_global` rather than `global` because the behaviour is a fallback — current module first, then the
global registry — and `get::<T>().global()` reads as "get the global `T`".

**The framework owns two meanings of `global` deliberately, and only two.** *Visibility*: a module's
exports reach everyone (`#[module(global: true)]`, `is_global`, `global_providers`). *Application*:
an enhancer runs on every dispatch target (`use_global_http_guards`). Search is not a third — after
the narrowing it is a fallback into the first, which is what the new name says.

**Revised by the user, 2026-09-29, while building row 40.** The reach is what the module could
inject: its own providers, what its imports export into it, then what the global modules export, the
loader's order, with a key two imports export answering `AmbiguousModule`. That puts an import's
non-global export inside the set, so the `or_` rationale above ("current module first, then the global
registry") no longer describes it and `or_global` names one of three tiers. The name is
`.visible()`, reasoned by Fable: the set is what the module can see, *visibility* being the first
meaning of `global` above; the bare adjective is the house form for a mode; and "get the visible `T`"
reads correctly for every tier, which "get the global `T`" did not. No `strict` option or wording:
that is Nest's name for a flag ulo spells as the default versus `.visible()`. ADR-0057 carries both.

### D2 — what a duplicate token registration means — **refused, and the multi thread is pulled in**

A token is single-bound unless declared multi. A second single binding is refused, naming both. A
multi token collects, in declaration order.

Rejected: refusing only the framework-fixed tokens; and defining last-wins as an override mechanism,
which nothing documents and which the `APP_GUARD` case cannot express anyway.

**This merges `project-multi-provider-redesign` into this build.** The two halves are one rule:
"many under one token" is a declared intent, a clash is an undeclared collision, and the second is
refusable only once the first works as ordinary token semantics. `APP_GUARD` is then a multi token by
definition and F138's guard-runs-twice defect stops existing rather than being special-cased. §9.1's
measurement is the other half — `multi` is one modifier, not five re-implementations.

The pipeline build's PR 5 replaces the globals registration surface and must carry this answer.

### D3 — is lifecycle ordering a contract? — **yes**

**Construction order is dependency order; ties break by declaration order; hooks follow construction;
shutdown is the exact reverse.**

Ties break by declaration order because that is the only order the author wrote and can read. The
precise rule is *depth-first over the declaration list*: walk the list in order, and before emitting
a provider emit everything it injects. Written in a valid order, a module gets back exactly what it
declared.

The design is stated first and the storage follows from it. An insertion-ordered map with the same
hasher is the fit; `FxHashMap` is a speed choice for string keys that promises no order, which is
correct for a pure lookup and wrong once iteration carries a contract. Four maps need it —
`Module::providers`, `Module::providers_instances`, `Module::controllers`, `Container::modules`. The
`TypeId`-keyed three in F45 are lookups only and stay.

Rejected: leaving it unspecified and documenting that, which asks every application to encode an
order the container already computes.

### D4 — one token classifier — **yes**

Accept any path in token position, emit the `IntoToken` call, let the compiler reject what is neither
a `Token<T>` nor a `&str`, with `#[diagnostic::on_unimplemented]` carrying the message.

Rejected: patching each surface as it is reported, which leaves five surfaces with five subsets and
has already been rediscovered from two directions.

§9.2 supersedes the mechanism without changing the decision: once a token is a value rather than a
token tree, there is no classifier left to fix.

### D5 — `#[module]` export parity with the builder — **yes**

All four keys parse expressions; `exports:` gains the two forms `export::<T>()` and `export_token(s)`
have; `Extension<T>` implements the one declaration trait.

Rejected: documenting the constraint, which pushes an application needing a string-token export to
`DynamicModule`.

### D6 — `Arc<Box<dyn Provider>>` — **yes, take the break**

`Arc<dyn Provider>` throughout. The user's standing answer: an SPI break is allowed in any phase of
this work.

### D7 — sequencing — **the triage pass owns it**

The user runs every outstanding audit together before building — each audit grouped with its memo and
its build handoff in one folder, all folders under a project folder, then a triage pass over the set
deciding the build order and what can be built together.

So this memo recommends no sequence. The one coupling to carry forward: D2's answer belongs in the
pipeline build's PR 5, which would otherwise ship a globals surface that reopens F138.

### D8 — the declaration surface becomes values — **yes**

Three constructors, one accessor, two blanket modifiers, as prototyped in §9.2. Five proc macros go.

Rejected: tidying the five markers into one grammar, which leaves ten code paths and the token as a
token tree; and keeping both vocabularies with one documented as preferred, which is the state that
produced 148 call sites across two spellings.

### D9 — a provider factory is always async — **yes**

Rejected: a marker type parameter over sync and async, which is ambiguous at the call site for an
async closure (`E0282`), exactly as `project-marker-vs-autoref-disambiguation` predicts for two
open-ended bounds on one parameter. Rejected: two entry points, which compiled a provider of futures
when the async form was written into the sync one, until an `Output: Clone` bound was added — and
still leaves two doors for one act.

The argument the user made, and it is the general one: **Nest can accept a value or a promise from
`useFactory` because JS `await` on a non-promise is a no-op.** Rust has no such affordance, and
copying the duality means building the affordance by hand. Every cost above is the price of
building it. The duality is the weak part of the borrow.

Cost: 28 of 32 `provider_factory!` sites in the tree are sync and gain one word. `Provider::resolve`
and `ProviderFactory::build` are already async, so the surface becomes consistent with the SPI rather
than paying a tax.

Implementation note: `AsyncFn` as a bound accepts every async spelling and rejects sync cleanly, but
its associated future type is unstable, so `Send` cannot be named on it and `#[async_trait]` requires
`Send`. The stable form `F: Fn(A..) -> Fut, Fut: Future<Output = R> + Send` accepts the same set and
is what was validated.

### D10 — `#[new]` is the only constructor — **yes**

`init = "…"` is removed from `#[injectable]`, and `#[controller]`'s refusal message goes with the key.

`#[new]` reads dependencies from the signature, where a Rust reader looks, and needs no attribute on
the fields. All five `init = "…"` uses in the tree are `init = "new"`. `#[controller]` already made
this choice; removing the key is what makes its diagnostic true.

### D11 — the naming vocabulary — **as tabled in §9.4**

`Injectable` → `Registration`; one accessor `T::provide()`; `.under(token)`; `.multi::<Tr>(base)`;
`.or_global()`. `Provider`, `ProviderFactory`, `#[injectable]` and `#[inject]` unchanged.

### D12 — one provider method on the builder — **yes**

`.provider_factory(f)` and `.provider::<T>()` collapse into one `.provider(value)`, and the same for
the controller pair. `#[module(providers: [...])]` and the builder then take the same expressions —
one vocabulary, two syntaxes for one list. `__ulo_provider_factory` leaves the public surface.

### D13 — pre-release: no deprecation shims — **yes**

The crate is unpublished and the first release will be a beta, so nothing in this build is retained
for compatibility. `provide!` and the four macros are deleted rather than deprecated, `init = "…"` is
deleted rather than warned on, `Injectable` is renamed rather than re-exported under both names, and
`.global()` gains no alias.

The rule is about keeping an API alive, not about keeping a diagnostic that explains a rename. For
`init = "…"` no migration error is kept either: the audience pre-release is this repo's five call
sites and the docs site, both of which the change updates. Whether the migration errors already in
the tree should be swept for the same reason is a separate call and is not folded in here.

### D14 — an injectable is shared, not copied — **`Arc` for singleton and execution, by value for transient**

*Settled 2026-09-22, raised by the extraction audit rather than by this one. It resolves
`project-injectables-by-value-vs-arc`, parked 2026-08-28.*

A provider hands out a **shared** instance for singleton and execution scope. Transient keeps the
current behaviour, where a fresh value per resolution is the point.

**What is wrong today is not a cost, it is a promise.** The scopes are read as statements about
identity and they are statements about construction count. A singleton is built once and then
*copied* to every injection point — the generated provider holds `instance: Arc<T>` and answers
`Box::new((*self.instance).clone())` (`crates/ulo-macros/src/provider_macro/instance_injection.rs:363`,
`:372`). Execution scope is the same one scale down: `ExecutionCache` stores
`Arc<dyn Any + Send + Sync>` and `get` ends in `.cloned()`
(`crates/ulo/src/di/execution_cache.rs:39-52`). So two services injecting one execution-scoped
provider in one request hold two of it, and the workspace `CLAUDE.md` states the consequence as the
model — *"one construction per execution, shared by clone — mutable state needs `Arc<Mutex<_>>`"*.

**The `Arc` is already there, in both layers, and discarded at the boundary.** This is not adding an
indirection; it is keeping the one that exists. `Arc` is also already the vocabulary the rest of the
surface uses — `Vec<Arc<dyn Trait>>` for a multi-provider, `Arc<dyn Middleware>`, and dispatch
targets since #197.

**The spelling: the field is written `Arc<T>`.** `#[inject] svc: Arc<OrdersService>`, explicit at
the injection site.

*Rejected: the macro rewriting the field.* Storing `Arc<T>` behind a field the author wrote as `T`
and reaching it through `Deref` keeps every existing signature compiling and makes the sharing
invisible, which is the property this decision exists to make visible. It also leaves by-value use
of the field failing at a type the author never wrote.

*Rejected: leaving the model and documenting it.* That is the state today. It is documented, and the
documentation is a workaround notice rather than a description — `Arc<Mutex<_>>` is advice about how
to defeat the copy.

**What it carries with it, and what is not optional.** The `#[derive(Clone)]` the provider macro adds
to every injectable (`instance_injection.rs:237`) goes, and with it the requirement that every field
of an injectable be `Clone`. Shipping the sharing and keeping the derive pays the migration for half
the benefit, and leaves F7's diagnostic in place. Transient keeps a by-value resolve, so the macro
carries two shapes selected by scope — that branch is the one a test has to cover.

**The verification, which is the decision made executable.** Three assertions in one test: two
providers injecting one **singleton** hold the same object (`Arc::ptr_eq`); two providers injecting
one **execution-scoped** provider inside one request hold the same object; two providers injecting
one **transient** provider do not. The first two fail today and are what this changes; the third is
the falsification and must keep passing.

**Two things downstream of it**, neither of which is this build's to sequence:

- `ulo::http::Request` has nothing left to carry once injectables are shared — `Arc<RequestPart>` is
  the whole of it. F180.
- `Extension<T>` holds an `Extensions`, which is already `Arc`-backed, so it must not be wrapped
  again. Decided with this: **`ExtensionFactory<T>` is deleted** and the container answers any
  `Extension<T>` token from the execution's bag, the way it already answers `Extensions`. One
  open-ended rule in place of one registration per payload type. F181.

---

## Out of scope

- **Dispatch and the enhancer pipeline** — the pipeline audit's, per the handoff. Where a finding here
  touches them (§1's globals half, F73) it says so and defers.
- **Extraction, the wire, rendering** — each has its own handoff.
- **The six parked threads** — `project-injectables-by-value-vs-arc`,
  `project-multi-provider-redesign`, `project-module-as-di-participant`,
  `project-lazy-loading-modules`, `project-behavioral-traits-audit`,
  `project-provider-context-variant-criterion`. All six were read. **One is no longer parked:** D2
  pulls `project-multi-provider-redesign` into this build, because refusing a duplicate token and
  making "many under one token" work are one rule. Its own sketch — multi as ordinary token semantics
  rather than a side-channel — is what §9.1's measurement independently arrives at. The other five
  stand, and nothing here proposes inside them. One note for whoever picks them up: the owed
  `ProviderContext` doc edit's target moved to `crates/ulo/src/di/execution.rs`, where the criterion is
  still unstated.
- **Measuring anything.** The `Arc<Box<…>>` hop, the `IdHasher` in F45, and the linear scans in the
  scanner and the module sort are the performance pass's.

## Bookkeeping owed

**F38 and F80 are one finding filed twice** — clippy absent from CI, the same `never_loop` error in
the same file, recorded against two commits three weeks apart. A cross-reference was added to F80
rather than deleting either, F38 carrying the 190-warning backlog and the clean-worktree
reproduction that F80 does not. Outside this pass's subject and noticed while checking a citation.

The workspace `CLAUDE.md` names `ProviderContext` twice and `provider_scope` once. Both were renamed
by `5548518c` (2026-09-14, ADR-0047) — `di::Execution` and `di::scope` — after the last documentation
sync at `fd9dc5e`. Correcting a falsified row belongs to the change that falsified it; these were
missed, and the DI build's first PR to touch the row should take them.
