# The core model of ulo's dependency injection

Written 2026-10-01 for a reviewer who knows Rust and dependency injection and has not seen ulo.
Every section states what is decided, the reason in a sentence, and what was rejected, so a
reviewer can argue with one claim at a time. Each part carries one of three tags:

- **built today** — in the tree at the tip of the current stack (branch
  `refactor/read-an-extension-from-the-bag`, PRs #392 and #393 open).
- **rebuild** — changes in the rewrite of #392/#393 the user decided on 2026-10-01: every
  dependency wrapped, `Dep<T>` the site type.
- **later (F-number)** — deferred, filed in the framework-gap ledger under that number.

Claims are marked **Probed** (a scratch crate compiled or ran the shape), **Read** (the tree says so)
or **Asserted** (reasoning from the two). The appendix lists the probes.

ulo is a NestJS-shaped framework: modules with imports and exports, providers with scopes,
controllers as dispatch targets on four transports (HTTP, WebSocket, RPC over seven brokers or
sockets, gRPC), and enhancers (guards, interceptors, error handlers) around every handler. This
document covers the container under all of that.

## 0. The model in five sentences

1. **A key is a type.** A binding is stored under the name of a type, and a type is its own key;
   a second slot for a type is a marker type that says what it holds.
2. **A binding answers one shared instance.** Every provider, whatever declared it, answers an
   erased `Arc<T>`; a transient answers a fresh one per site.
3. **A site is a `Dep<T>`.** A field, a constructor parameter and a factory parameter are one kind
   of thing, keyed and filled by their type alone; `Vec<Dep<T>>` reads a collection and
   `Extension<T>` reads the execution's bag.
4. **A module is a visibility boundary.** A binding reaches another module through an export or a
   global module. A question two imports answer is refused, never guessed; one a nearer tier
   answers — the module's own binding over an import's, an import's over a global's — is answered
   by that tier (Probed Pv1–Pv3; whether the order stays is F290).
5. **An execution is a cache.** One HTTP request, one WebSocket message, one RPC or gRPC call, or
   one standalone run; an execution-scoped binding is built once per execution and shared inside
   it.

## 1. The nouns

### 1.1 Keys — built today

A key to the container is a type. Storage is the type's fully qualified name (`type_name`), which
the compiler canonicalizes, so an alias, a qualified path and a renamed import all name one slot.
`Db` is its own key with nothing to write. A second slot for a type, or a slot for a role, is a
marker type implementing `Key`:

```rust
pub trait Key: 'static {
    type Value: ?Sized + 'static;   // what the slot holds
}

key!(pub Replica: Db);                   // a second `Db`
key!(pub Auth: dyn Guard<HttpContext>);  // a role slot: whatever is bound here is a guard
key!(pub Main: sqlx::PgPool);            // a foreign type named by a marker
```

`#[injectable]`, `#[controller]` and `#[websocket_gateway]` implement `Key` for their type with
`Value = Self`; any other type of the crate takes `#[derive(Key)]`. A blanket `impl<T> Key for T`
would collide with every marker (E0119), which is why the reflexive impl comes from the attribute.

*Reason:* a string key is unchecked and a const beside a type is ambiguous to a macro (`tokens::AUTH`
and `guards::AuthGuard` are both paths); a type is the one thing every key position can read.
*Rejected:* Nest's three key kinds (class, string, symbol); a position rule telling a const from a
type by casing; a `Named<"db">` const parameter, unstable.

**Trait objects.** `dyn Logger` is a type, and a key: `provide!(dyn Logger => ConsoleLogger)`
binds it and a site reads it. A trait object is held behind an `Arc` by a provider shared across
threads, so the trait needs `Send + Sync` as supertraits. `dyn Repo` and `dyn Repo + Send + Sync`
are two types with two names even when `Repo: Send + Sync` is declared (Probed P5), and they are
two keys. The rule is one spelling, `dyn Repo`, with the supertraits on the trait. **Rebuild:** the site
refuses a trait object without the supertraits at compile time through `Dep`'s own bound (Probed
P8b), and a startup miss on `dyn X + Send + Sync` where a slot exists under `dyn X`, or the reverse,
names the other spelling in the error. *Rejected:* refusing the `+ Send + Sync` spelling syntactically, which an alias
defeats; there is no type-level way to strip auto-trait bounds from a `dyn` type. The framework's
own role slots accept both spellings for the `Arc<dyn Guard<C> + Send + Sync>` idiom, and keep
that tolerance.

### 1.2 Bindings — built today, one clause rebuild

A binding is a value the container holds under a key. Every form below is a value the public
constructor of the value API also builds, and `provide!(key => source)` is sugar over those
constructors, so a `#[module(providers: [..])]` list, a `DynamicModule` builder and an integration
crate's `for_root` function build the same values.

| Form | Written | Answers |
| --- | --- | --- |
| a type's own declaration | `Db` (a bare path), or `Db::provide()` | the instance `#[injectable]` builds, under `Db` |
| a value | `provide!(Port => 3000u16)`, `Provide::value(v)` | one shared instance, built where written |
| a factory | `provide!(async \|cfg: Dep<Config>\| Pool::open(&cfg.url).await)`, `Provide::factory(f)` | one shared instance, built once at startup; `.per_execution()` and `.transient()` rescope it |
| an alias | `provide!(Legacy => alias dyn Logger)` | whatever the named binding answers, under a second key |
| a trait-object slot | `provide!(dyn Logger => ConsoleLogger)` | the implementation, as `Arc<dyn Logger>` |
| a collection contribution | `provide!(into dyn Plugin => A {})` | one more item of the collection under `dyn Plugin` |
| a role slot | `provide!(Auth => HeaderGuard("x-auth"))` | the value, registered as a guard because `Auth::Value` is `dyn Guard<HttpContext>` |

Rules that follow:

- **A key is single-bound unless declared multi.** A second single binding under one key fails
  `create` naming both; a single binding beside an `into` contribution fails the same way. A
  collection is declared by contributing to it, keyed by its element type, and a second collection
  over one trait is a marker whose `Value` is that trait object. *Reason:* the old container let the
  second binding replace the first with nothing reported. *Rejected:* last-wins as an override
  mechanism, undocumentable and unable to express "two global guards".
- **A role comes from a type**, the enhancer's own trait impls or the key's `Value`; a value under
  a data-typed marker registers no role. *Reason:* a generic constructor cannot probe a value's type
  for a trait on stable Rust. *Rejected:* a `.guard::<Http>()` modifier per role; a `#[module]` DSL
  that sees text, not types.
- **A factory is always async.** *Reason:* Nest accepts a value or a promise because `await` on a
  non-promise is a no-op; Rust has no such affordance, and building it by hand is either ambiguous
  at the call site or two doors for one act.
- **A factory's parameters are its dependencies**, one per parameter, keyed by the parameter's
  type. Today they are `Arc<Dep>` and are not injection sites (a `Vec<Arc<dyn Plugin>>` parameter
  keys the `Vec`, F255). **Rebuild:** a factory parameter is a site like a field, and takes
  `Dep<dyn Logger>`, `Vec<Dep<dyn Plugin>>` or `Extension<T>` (Probed P8; §3.3).
- **The `Extension<T>` view needs no declaration.** Every execution carries an extension bag (what a
  guard writes for a handler to read). A site typed `Extension<T>` is keyed by the bag itself,
  which the framework registers globally, and views it as `T`. *Reason:* the container answers a
  `Box<dyn Any>` it downcasts by a key it knows as a string, so it cannot build an `Extension<T>`
  for a `T` it never sees; the site can. *Rejected:* one registration per payload type.
- **Collections gather across modules regardless of exports** (F281, open design question); a
  single binding does not.

### 1.3 Modules — built today; identity by type later (F286)

A module declares imports, providers, controllers and exports, and may be global:

```rust
#[module(
    imports: [DbModule::for_root(url), AuthModule],
    providers: [Orders, provide!(dyn Logger => ConsoleLogger)],
    controllers: [OrdersController],
    exports: [Orders, dyn Logger],
)]
pub struct AppModule;
```

**Identity.** A module has one identity: a base plus an optional config fingerprint, rendered
`base#hex`. For a `#[module]` struct the base is its type name; for a `DynamicModule` built by a
function such as `SqlxModule::postgres(url)` it is the builder-given string today. The fingerprint
folds configuration in: two imports of one maker with one URL dedup as a diamond, and two with
different URLs stay distinct and are refused where their exports clash globally. *Reason:* a
config-carrying module used twice is two modules, and a type cannot say which. **Later (F286):**
every `for_root` returns a nominal type, the key names a type path everywhere and `TypeId`
dedup replaces `type_name` dedup; the fingerprint stays, because `DbModule { url: a }` and
`DbModule { url: b }` are one type (Asserted). `TypeId` beside the name tells apart what one
`type_name` collapses, two versions of one crate in a graph; a keyed constructor returning
`DbModule<K>` is distinct by `TypeId` alone (Asserted, `MODULE_VISIBILITY_REVIEW.md` §2e).

**Visibility.** What a module can see, in the order the loader resolves an injection: its own
providers, then what each direct import exports into it, then what the global modules export. The
order is precedence: a key two tiers answer is answered by the nearer with nothing reported (Probed
Pv1–Pv3, `MODULE_VISIBILITY_REVIEW.md`; whether that order stays is F290). The runtime lookup
`module_ref.get::<T>().visible()` reads the same set in the same order. A binding exported neither
to the module nor globally is unreachable from outside its module, and an export reaches the
module's direct importers only (Probed Pv5a); an `exports:` entry the module imports rather than
provides is accepted at scan and stalls `create` (Probed Pv5b, F291). A key two imports export into
one module fails `create` naming both when something there injects it; where startup cannot see the
ambiguity, a lookup answers `AmbiguousModule` naming the candidates. `create` names one refusal,
the first the build reaches (Probed Pv4, F292). *Reason:* map order was
deciding which of two modules answered, and renaming a builder id flipped it. *Rejected:* a
search-everywhere lookup (`.global()` reached private providers of unrelated modules); the global
registry alone (misses an export the module could inject).

**Two meanings of global, and only two.** Visibility: a global module's exports reach every module.
Application: an enhancer contributed to a transport's global collection runs on every dispatch
target. Search is not a third.

### 1.4 Scopes and executions — built today

Three scopes, read as promises about identity, not construction count:

| Scope | Built | Shared by | Hooks |
| --- | --- | --- | --- |
| singleton (default) | once, at startup, outside any execution | every site in the application | run |
| execution | once per execution, inside it | every site in that execution | never fire; refused at compile time |
| transient | at every resolution | nothing; each site holds its own | never fire; refused at compile time |

An **execution** is one HTTP request, one WebSocket message (the connection is not an execution;
per-connection state is the session), one RPC call, one gRPC call, or one standalone run
(`Execution::standalone()`, for a job or a test). Each carries an `ExecutionCache`: one `Arc` per
type, first insert wins, so every site in the execution holds one instance. A singleton cannot
inject an execution-scoped binding: there is no execution to build it in, and `create` refuses it
with `ExecutionRequired`. A dispatch target that injects an execution-scoped binding is elevated to
execution scope and built per call; a per-execution guard or interceptor is built per call, each
guard only after the one before it admitted.

*Reason for one shared instance:* the previous model built a singleton once and copied it to every
site, so two holders of one execution-scoped provider in one request held two of it, and every
field of an injectable had to be `Clone`. *Rejected:* keeping copies and documenting `Arc<Mutex<_>>`
as the workaround; a macro storing `Arc<T>` behind a field written `T`, which hides the sharing the
decision exists to make visible.

## 2. `Dep<T>` — rebuild

### 2.1 Definition

```rust
pub struct Dep<T: ?Sized + Send + Sync>(Arc<T>);

impl<T: ?Sized + Send + Sync> Dep<T> {
    pub fn as_arc(&self) -> &Arc<T>;
    pub fn into_arc(self) -> Arc<T>;
}
impl<T: ?Sized + Send + Sync> Deref for Dep<T> { type Target = T; }
impl<T: ?Sized + Send + Sync> Clone for Dep<T> { /* Arc clone */ }
impl<T: ?Sized + Send + Sync> From<Arc<T>> for Dep<T> {}
impl<T: ?Sized + Send + Sync> fmt::Debug for Dep<T> { /* writes `Dep<type_name::<T>()>` */ }
```

The bound `Send + Sync` on the definition is what refuses `Dep<dyn Repo>` for a `Repo` without the
supertraits (Probed P8b). `Debug` names the key rather than the value, so a holder of
`Dep<dyn Repo>` derives `Debug` without `Repo: Debug` (Probed P6). No `Default`, no `PartialEq`, no
`Serialize`: a dependency is not data.

*Reason for a newtype over `Arc<T>` rather than `Arc<T>` itself:* mechanically equal at a site
(Probed P1b, P8); the user chose the noun, the `Debug`, and a representation the framework can
change later without touching a signature. *Rejected:* an enum `Dep<T> { Shared(Arc<T>), Value(T) }`
holding a handle by value, which cannot be `?Sized` and so cannot hold a trait object (Probed
P1a); a `Handle`/`Shared` marker trait letting a plain `T` field read a handle, which is E0119
beside `Arc<T>` and E0117 for a foreign pool type; the macro rewriting a field's type. The cost
accepted: a handle such as a pool that is already an `Arc` inside sits behind one more pointer, and a
library that takes an `Arc` gets it through `into_arc()`.

### 2.2 What every provider answers

A provider is the runtime object under a key:

```rust
#[async_trait]
pub trait Provider: Send + Sync {
    fn token(&self) -> String;
    async fn resolve(&self, ctx: Execution) -> Result<Box<dyn Any + Send>, ResolutionError>;
    fn scope(&self) -> ProviderScope;      // Singleton by default
    // five lifecycle hooks, no-ops by default
}
```

**Rebuild:** the erased answer is always an `Arc<T>` for the key's `T`. A singleton answers the one
built at startup; an execution-scoped provider answers the execution's cached one, building it on
first ask; a transient answers `Arc::new(fresh)`; a hand-written provider for a handle answers
`Arc::new(pool.clone())`. Today a hand-written provider answers the handle by value and the site
has an autoref ladder telling a shared answer from a value one, with a `Shape` the provider reports,
a `value_dependencies` list every factory reports, a `SharedByValue` refusal and a compile-time
`__ULO_SHARED` flag, all so that a plain `T` field could read a handle. Uniform wrapping deletes
every one of those (§5). *Reason:* the split between "shared" and "by value" is in the answer, not
in the field's spelling, and a generic site cannot straddle it (Probed P1c, P1d); once every answer
is an `Arc`, one impl per site shape suffices (Probed P3a, P8).

`Dep<T>` never crosses the erased boundary: it is the site's view, built from the `Arc` the site
downcasts. The value API's internals (`Arc<V>` to `Arc<dyn Trait>` casts, `Registration`,
`ProviderRole`) keep speaking `Arc`.

### 2.3 The site trait

One trait decides, for a site of type `S`, the key it reads, how it takes the erased answer, and
what `#[inject(K)]` checks. It replaces the autoref rung ladder of today:

```rust
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not an injection site",
    note = "a dependency is written `Dep<T>`, a collection `Vec<Dep<T>>`, a view `Extension<T>`; \
            a trait object needs `Send + Sync` as supertraits"
)]
pub trait Site: Sized + 'static {
    type Held: ?Sized + 'static;                       // what the slot holds
    fn key() -> String;                                // token_of::<Held>() unless a view
    fn take(answer: Box<dyn Any + Send>, token: &str) -> Result<Self, ResolutionError>;
    fn inject_key<K: Key<Value = Self::Held>>() -> String { token_of::<K>() }
}

impl<T: ?Sized + Send + Sync + 'static> Site for Dep<T>       { Held = T;  take: downcast Arc<T> }
impl<T: ?Sized + Send + Sync + 'static> Site for Vec<Dep<T>>  { Held = T;  take: each item's Arc<T> }
impl<T: 'static>                        Site for Extension<T> { Held = Extensions; take: the bag, viewed as T }
// later (F289): impl<P: Site> Site for Option<P> { Held = P::Held; key = P::key(); a miss answers None }
```

The macros emit `<#ty as Site>::key()` and `<#ty as Site>::take(..)` at every field, constructor
parameter and factory parameter; `IntoFactory` bounds each parameter by `Site` (Probed P8: a
factory of `Dep<dyn Repo>`, `Vec<Dep<dyn Plugin>>` and `Extension<u8>` keys `dyn Repo`,
`dyn Plugin`, `Extensions`). A field whose type is none of these fails to compile at the field with
the note above (Probed P3b's shape). *Rejected:* keeping the ladder and adding `Dep` as one more
rung, which keeps the plain-site machinery for nothing.

The reading is by type, not spelling: `type Store = Dep<Repo>` and a renamed `Dep` read the slot
`Repo` (this is what #392 already guarantees for `Arc`, and the trait keeps it).

### 2.4 Lookups

`get::<T>()` and `resolve::<T>(&execution)` on the application, the application context and a
`ModuleRef`, and their `_key::<K>` forms, answer `Dep<T>`. `get` runs outside any execution, so an
execution-scoped binding answers `ExecutionRequired`; `resolve` takes the execution to build in.
`resolve::<Extension<T>>` is not a lookup: a lookup is generic over `T` and the container has no
provider under that key; code outside the injection tree reads the bag with `resolve::<Extensions>`
(§7, Halt 9's lookup half).

### 2.5 What the user writes

```rust
#[injectable]
pub struct Orders {
    #[inject] db: Dep<Db>,                       // a service: one shared instance
    #[inject] pool: Dep<PgPool>,                 // a handle: the same spelling
    #[inject] draft: Dep<Draft>,                 // a transient: fresh at this site
    #[inject] logger: Dep<dyn Logger>,           // a trait-object slot
    #[inject] plugins: Vec<Dep<dyn Plugin>>,     // a collection
    #[inject] user: Extension<CurrentUser>,      // the execution's bag, viewed as CurrentUser
    #[inject(Replica)] replica: Dep<Db>,         // a marker's slot; checked: Replica::Value = Db
    #[default(3)] retries: u32,                  // owned state, until F288 moves it to the constructor
}

impl Orders {
    pub fn spawn_sync(&self) {
        let db: Arc<Db> = self.db.as_arc().clone();     // a library boundary that wants an Arc
        tokio::spawn(async move { db.flush().await });
    }
}

let orders: Dep<Orders> = ctx.resolve::<Orders>(&execution).await?;
```

`#[inject]` today marks a field as a dependency; an unmarked field is owned state (`Default` or
`#[default(expr)]`). **Later (F288):** the struct form injects every field, and bare `#[inject]`,
`#[default]` and the `Default` fallback are retired; `#[inject(K)]` stays as the one field helper
(§3.1, §7).

## 3. Construction

### 3.1 The two forms — struct form built today; impl form later (F288)

**Today** the attribute goes on the struct and a constructor is a method marked `#[new]` in any
inherent impl. The struct macro cannot see the impl, so `#[new]` emits an inherent const that
out-ranks a blanket default (the autoref bridge), and the struct macro reads it at a concrete site:
present, build through the constructor; absent, build from the fields. Dependencies of a
`#[new]` are read from its signature, and its struct needs no field attributes; `#[default]` beside
`#[new]` is refused, the constructor setting every field; two `#[new]` collide on the one const.
A bare `new()` is not detected (Probed: it is ignored and the instance is field-built).

**Later (F288)**, the impl becomes the promoted form:

```rust
pub struct UserService {            // untouched: no attribute, no field helpers
    db: Dep<Db>,
    uow: Dep<UnitOfWork>,
    retries: u32,
}

#[injectable(scope = "execution")]
impl UserService {
    pub fn new(db: Dep<Db>, uow: Dep<UnitOfWork>) -> Self {    // auto-detected; `#[construct]` names another
        Self { db, uow, retries: 3 }
    }
}
```

Rules the F288 ADR carries: the impl form reads the constructor's parameters and never the fields;
the struct form injects every field; both forms on one type fail as conflicting impls of the
generated traits (E0119, Probed P2c's shape); `new` is auto-detected only in the impl form, because
the struct macro cannot see a method and a bridge needs a marker; `#[construct]` replaces `#[new]`,
is stripped inside an annotated impl and emits the bridge outside one; hooks in another impl keep
the lifecycle bridge; bare `#[inject]`, `#[default(expr)]` and the `Default` fallback are retired,
leaving `#[inject(K)]` on a field or a parameter. *Reason for the split:* once owned state lives in
the constructor, a field helper has nothing to say but the key, and "every field is a site" cannot
be enforced conditionally on a constructor the struct macro cannot see (Asserted).

**Fallible, async and optional constructors — later (F289).** Today `-> Result<Self, E>` and
`async fn` fail to compile inside the `#[new]` expansion (Probed, E0271 both), and
`Option<Dep<T>>` is keyed by its own type and fails `create` (Probed). Each is one small row: a
fallible constructor's return converted through an autoref pair (never by reading the return type's
name, which an alias defeats), an async one detected from `sig.asyncness` (a token, not a spelling),
an optional one as the `Option<P>` site impl plus an optional flag in the dependency list, because
the loader refuses a missing key before any site takes.

### 3.2 The generated machinery — built today

For every annotated type the macro emits a `ProviderFactory` (the recipe: key, dependency keys,
`build(deps) -> Registration`) and a `Provider` (the instance and its scope). `Registration` is the
instance plus the roles it registers under: enhancer roles are detected from the type's trait impls
at the concrete site (`impl Guard<HttpContext> for AuthGuard` is the declaration), the gateway role
from the structural macro. *Rejected:* marker attributes such as `#[guard]`.

### 3.3 Factories and the value API — built today; parameters as sites in the rebuild

```rust
Provide::factory(async |cfg: Dep<Config>, log: Dep<dyn Logger>| Pool::open(&cfg.url).await)
    .per_execution()          // or .transient(); default: a singleton built once at startup
```

`IntoFactory<A>` is implemented per arity for `Fn(A1, .., An) -> Fut` with `Fut: Future + Send`;
each `Ai: Site` (**rebuild**), so a factory parameter reads exactly what a field reads: a trait
object by its trait, a collection by its element, a view by the bag. This closes F255 and Halt 9's
factory half. A factory's output is bound under the type it builds, or under a marker through the
`_key` forms.

### 3.4 How a third-party type enters — built today; nominal module types later (F286)

Two ways, both ordinary bindings:

1. **A factory**, in an application module: `provide!(async |cfg: Dep<Config>| PgPool::connect_lazy(&cfg.url))`,
   bound under `sqlx::PgPool`; a second pool goes under a marker, `provide!(Replica => async |..| ..)`.
2. **An integration crate's hand-written provider** behind a `for_root` function. `SqlxModule::postgres(url)`
   builds a `DynamicModule` whose providers are the crate's own `Provider` impls (a pool provider
   answering `Arc::new(pool.clone())` in the rebuild, a health indicator), exports the pool's key,
   and wraps a startup reachability check the caller may weaken (`.without_startup_check()`). The
   application writes `imports: [SqlxModule::postgres(url)]` and `#[inject] pool: Dep<PgPool>`.
   **Later (F286):** the function returns a nominal type, `SqlxPostgres { url, check }`, and the
   module's identity is that type plus the fingerprint.

The orphan rule keeps an application from implementing `Key` for `PgPool`, so a marker names it
where a key position needs one; a bare `Dep<PgPool>` site needs no marker.

## 4. The lifecycle end to end — built today

**Create.** `UloFactory::create(RootModule)`:

1. *Scan.* Walk the module graph from the root, depth first, deduplicating on identity keys;
   register each module's provider factories, controller factories, exports, global exports and
   collection contributions. Refuse a key bound both as single and as a collection.
2. *Build, in passes.* For each module, a per-module dependency graph sorts its providers
   topologically, ties broken by declaration order (a module written in a valid order gets back
   what it declared), collection contributors ordered before the collection's consumers. Each
   factory's dependency keys are resolved: local instances, then a pre-registered instance
   (`ModuleRef`), then one import's export (two exporters: refused naming both), then the global
   registry, then a collection. A dependency a later module will contribute globally defers the
   module to the next pass; a pass that builds nothing turns the deferrals into the stall
   diagnostic, which names the exact provider cycle when there is one. `build` is awaited outside
   the container lock, because a constructor may itself resolve.
3. *Assemble.* Collections are gathered into one provider each; the `ModuleRef` store is filled;
   the HTTP global collections become HTTP's global enhancers; middleware tokens resolve;
   controllers build last and see every enhancer.
4. *Hooks.* `on_module_init` then `on_application_bootstrap`, each walking construction order
   (dependency order: a provider's hook runs after the hooks of what it injects), the module's
   own hook ahead of its providers'. A failing hook is `StartupError::HookFailed`. A dispatch target
   built per call fires init and bootstrap on each instance it builds, before the cache holds it.

`create` returns `Result<_, StartupError>`; nothing exits the process. Every DI defect above is a
returned error, and the rule is that none leaves `create` by panic.

**Bind, run.** Registration for every transport completes before any socket is acquired, so a
misconfiguration reports before a port conflict; any transport failing `bind` closes what was
acquired and leaves the application in a terminal state.

**A call.** The adapter opens an execution (a context handle carrying the cache, the extension bag,
the cancellation token and what the transport carries) and hands it to the pipeline. On HTTP: the
global middleware chain before routing, the module's middleware after it, then guards,
interceptors, handler; on the other three, guards, interceptors, handler. Each enhancer entry is
either an instance every call shares or a factory asked for one in this execution; a guard is built
only after the previous guard admitted, and nothing below the guards is built until they all have.
The dispatch target resolves in the execution too: a singleton controller answers the startup
instance, an elevated one is built into the cache on first ask and shared by everything in the call.
Every `Dep<T>` site in that tree resolves against the same cache, so an execution-scoped binding is
one object however many holders it has. The execution ends when the context drops.

**Close.** Three phases, each walking construction order in reverse, the module's own hook first in
each: `on_module_destroy`, `before_application_shutdown(signal)`, `on_application_shutdown(signal)`.
A serve loop that dies closes the whole application through the same drain; there are no built-in
signal handlers.

## 5. What is refused, and where

| Where | Refusal | Tag |
| --- | --- | --- |
| compile | a key position given a string, or a type not implementing `Key`; `#[inject(K)]` over a site whose `Held` is not `K::Value` | built today |
| compile | a site whose type is not `Dep<T>`, `Vec<Dep<T>>` or `Extension<T>` (later `Option<..>`) | rebuild |
| compile | `Dep<dyn Repo>` for a `Repo` without `Send + Sync` supertraits | rebuild |
| compile | a lifecycle hook on an execution-scoped or transient type | built today |
| compile | `#[default]` beside `#[new]`; two `#[new]` on one type | built today (retired with F288) |
| compile | two body-consuming extractors on one handler (`FromContext::CONSUMES`) | built today |
| compile | a `provide!` value under a marker whose `Value` it is not; a non-guard under a guard-typed marker | built today |
| `create` | a dependency nothing visible provides; two imports exporting one key into a module that injects it; two single bindings under one key; a single binding beside an `into`; a global export claimed by two distinct modules | built today |
| `create` | a provider cycle, named; a stall with no cycle, each module's unresolved key named | built today |
| `create` | a singleton injecting an execution-scoped binding or an `Extension<T>` view (`ExecutionRequired`) | built today |
| `create` | a miss on `dyn X + Send + Sync` where `dyn X` is bound, or the reverse, with the spelling named | rebuild |
| resolution | `ExecutionRequired` for an execution-scoped lookup outside an execution; `TypeMismatch`; `ProviderNotFound`; `AmbiguousModule` | built today |

**Disappears in the rebuild:** the plain-site machinery in full. `Provider::shape`,
`ProviderFactory::value_dependencies` and `ControllerFactory::value_dependencies`, the loader's
`refuse_shared_by_value`, `ResolutionError::SharedByValue`, the `__ULO_SHARED` compile-time flag
and its per-field `const` checks, `take_value`/`take_shared`, the `SiteValueVec` rung and the
five-rung autoref ladder, `Ctor::values`, the `value_reads` list, and two gap entries (F284, the
compile-time refusal reading a type and so refusing a plain field over a transient; F285, a sized
element type contributable but not injectable). Each existed only to let a plain `T` field read a
handle by value.

## 6. The later layer: type-level wiring — later (F287)

The model above checks the graph at `create`. F287 would check visibility at `cargo check`, on top
of F286's nominal module types and F288's forms, without replacing the runtime graph. The shape
with teeth (Probed P4a):

- each module type carries `type Exports` and `type Imports` as type-level lists, emitted from
  `exports:` and from the import expressions' nominal types;
- each injectable carries `type Deps`, the `Held` types of its sites, and each factory arity its
  parameter list;
- `Visible<T, Idx>` is derived over own exports and imports' exports with an *inferred index*
  (frunk's `Selector` shape; without it the derivation is E0119 with two imports, Probed P4b, and
  an index parameter cannot sit on a constructor's impl, E0207, Probed P4d, so the proposal's
  `impl<R: Resolve<Db>> Construct<R>` is not the vehicle);
- the root emits one never-called assertion per (consumer, module), walking imports transitively
  and passing the global modules' exports.

What it would check: a key nothing visible provides (E0277, with a message naming the key and the
module), before `create`. What it would not: ADR-0057's ambiguity, two imports exporting one key,
surfaces as E0283 "type annotations needed" with no way to name the modules (Probed P4c) — a
`macro_rules!` callback per `#[module]` listing its exports, invoked by the importer's macro to
`compile_error!` with both names, reaches only modules a macro wrote and compares exports as token
text (Asserted, `MODULE_VISIBILITY_REVIEW.md` §1) — so the runtime refusal stays the one that
explains; a provider registered from configuration at runtime is
not in `Exports`; two configs of one module type are one type; execution scope and async resolution
are untouched; the loader still walks the same graph. The cost is a type-level copy of the wiring in
every module and injectable. Recommendation on record: build F286 and F288 first; take F287 only if
the edit-check loop is what hurts.

## 7. Open decisions

Each is the user's; each carries a recommendation.

1. **Field helper naming (F288).** `#[inject(K)]` is the only helper left once bare `#[inject]`
   and `#[default]` go; it names a slot on a field or a constructor parameter. Recommend keeping
   `#[inject(K)]`: "inject `K` here" is what it does, and a crate-namespaced helper
   (`#[ulo(key = K)]`) or `#[injectable(K)]` on a field adds a spelling to say the same thing.
   `#[inject(default)]` in any spelling is not needed: an optional dependency is `Option<Dep<T>>`,
   a configured value is a binding, and everything else is the constructor's.
2. **Collection spelling.** `Vec<Dep<T>>`: the item is a dependency and the container is a `Vec`,
   and the site impl is on `Vec<Dep<T>>` directly (Probed P8). *Rejected:* a `Deps<T>` newtype,
   a second noun for one thing; `Dep<[T]>`, which cannot hold unsized items.
3. **What a transient answers.** `Dep<T>` over a fresh `Arc<T>`, unique to the site
   (`Arc::get_mut` through `into_arc()` gives `&mut` when needed). *Rejected:* a by-value transient
   read by a plain site, which brings back the plain-site machinery for one scope; a `Fresh<T>`
   type, a second noun.
4. **Halt 9's lookup half.** `resolve::<Extension<T>>` stays unsupported; code outside the tree
   reads `resolve::<Extensions>` and views it. *Rejected:* lookups generic over `Site`
   (`resolve::<Dep<T>>() -> Dep<T>`), which lengthens every existing lookup to reach one view;
   an optional declaration for the view, which re-creates the registration the model removed.
5. **F281, collections and exports.** Not decided by this document; the model states the current
   behaviour (§1.2) and the question stands.

## Appendix: sources and probes

Read against the worktree `/Users/ifeanyi/Projects/Personal/open-source/toni_rs/toni-rs-r33`
(`crates/ulo/src/__di.rs`, `crates/ulo/src/di/{provide,binding,collection,key,extension,execution,execution_cache}.rs`,
`crates/ulo/src/di/internal/{instance_loader,container,scanner,dependency_graph,module_ref}.rs`,
`crates/ulo/src/di/module/{identity,dynamic,checked,metadata}.rs`, `crates/ulo/src/spi/provider.rs`,
`crates/ulo/src/__construct.rs`, `crates/ulo/src/enhancer/pipeline.rs`, `crates/ulo/src/application_context.rs`,
`crates/ulo-macros/src/provider_macro/*.rs`, `crates/ulo-macros/src/shared/{site,inject_key}.rs`),
the ADRs 0057, 0058, 0059 in `docs/adr/`, and the DI audit's D14 with Halts 8 and 9 in
`PROJECT_SEPTEMBER_SMILE/BUILD_SEQUENCE.md`.

Probes, rustc 1.98.1, edition 2024, under
`/private/tmp/claude-501/-Users-ifeanyi-Projects-Personal-open-source-toni-rs-toni-rs/a632688a-d566-4ff7-b727-8f28e1dd90c6/scratchpad/fable-inject/`:

| Probe | Shows |
| --- | --- |
| P1a `std/p1a_inject_enum.rs` | a by-value-capable `Dep` cannot be `?Sized`: E0277 on `Dep<dyn Logger>` |
| P1b `std/p1b_inject_newtype.rs` | `Dep<T: ?Sized>(Arc<T>)` is `Arc`-sized and holds a handle as `Arc<Handle>` |
| P1c, P1d `std/p1c_*.rs`, `std/p1d_*.rs` | one generic take cannot wrap a by-value answer (E0277); a sized impl beside an unsized one is E0119 |
| P2c `std/p2c_two_frontends.rs` | two front ends emitting one impl: E0119 |
| P3a, P3b `std/p3a_*.rs`, `std/p3b_*.rs` | one site trait over `Arc<T>`, `Vec<Arc<T>>`, `Extension<T>` serves fields and factory parameters; a plain site is E0277 with the note |
| P4a–P4d `std/p4*.rs` | type-level visibility with an inferred index has teeth (E0277); two exporters is E0283; blanket derivation is E0119; an index on the constructor's impl is E0207 |
| P5 `std/p5_dyn_spelling.rs` | `dyn Repo` and `dyn Repo + Send + Sync` are two names |
| P6 `std/p6_derive_debug.rs` | `derive(Debug)` fails over `Arc<dyn Repo>` and passes over `Dep<dyn Repo>` |
| P7 `std/orphan/` | an application can implement a ulo trait neither for `Arc<App>` nor for `Dep<App>` (E0117) |
| P8, P8b `std/p8_dep_model.rs`, `std/p8b_dep_not_send.rs` | §2 end to end: the definition, the three site impls, `Option<P>`, a factory keyed by its `Dep` parameters, a collection taken from erased items, two sites over one answer sharing one allocation, `into_arc` at a library boundary; a non-`Send` trait object refused at the site |
| `ulo-probe/src/bin/*.rs` (against the worktree's `ulo`) | a fallible or async `#[new]` fails E0271; `Option<Arc<T>>` fails `create`; `#[inject(default)]` is E0425; `#[injectable]` on an impl is "expected `struct`"; a bare `new()` is ignored; a factory answering `Result<Pool, E>` binds the `Result` |
| P2d `std/p2d_fake_resolver.rs` | a partial fake resolver is E0277 under a `Construct<R>` bound; the bound's one use, which a `Dep<T>` constructor gives without a trait |

Visibility probes (2026-10-01, `MODULE_VISIBILITY_REVIEW.md`), under `../fable-visibility/src/bin/`
against the worktree's `ulo` at `95892262`:

| Probe | Shows |
| --- | --- |
| Pv1–Pv3 `pv1_local_beside_import.rs`, `pv2_local_beside_global.rs`, `pv3_import_beside_global.rs` | a key two tiers answer is answered by the nearer, at injection and through `.visible()`, with nothing reported |
| Pv4 `pv4_one_pass.rs` | `create` names one refusal: one module's, one key's, a miss ahead of an ambiguity |
| Pv5 `pv5_reexport.rs` | a transitive export is not visible; an `exports:` entry the module imports rather than provides stalls `create` |
