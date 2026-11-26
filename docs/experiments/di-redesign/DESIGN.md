# Dependency injection: end-to-end design

`fw` is a placeholder for the framework's crate prefix. Numbers in brackets like [12] refer to the capability list in the brief.

---

## 0. Principles

1. **One runtime container, validated as a whole graph before anything exists.** Every binding declares its injection points statically, so the complete dependency graph is known before a single instance is built. A hand-written `Construct` states its reads twice, and the first construction refuses a resolver read the declaration omits (§3.4), so such a read fails rather than escaping the checks wiring ran. All wiring errors are reported in one pass [44]. Anything that is local to one type (the type of an injection point, hooks, role bounds) is checked by the compiler instead [45].
2. **Macros are sugar.** Every macro expands to calls on the value-level API in §13, which integration crates call directly. A handler attribute lowers to `Mount::handler` with its transport's public handler value (transports X40), so a controller is written by hand as a provider is.
3. **Naming.** Types are nouns (`Dep`, `Many`, `Ext`, `ModuleRef`, `Execution`). A trait is named after its predominant method, as std's `Clone::clone` and `FromStr::from_str` are: `Construct::construct`, `FromContainer::from_container`, `Validate::validate`. A trait with no single method is named for its concept or role (`Iterator`, `Error`): `Module`, `Timer`, `Runtime`, `Server`, `Transport`, `Guard`, `Interceptor`, `ErrorHandler`. An error is named after its operation as users understand it, so `FromStr` fails with `ParseIntError` and `Construct` with `ConstructError`; after its domain when a family of operations shares it (`io::Error`; `LookupError`, which every resolver read produces); and for the condition it reports when it reports one rather than an operation's failure, the class std has in `PoisonError`, written in the past participle: `GuardRejected`, `PanicRecovered`, `RuntimeMissing`. `Closed` stays, its subject implied wherever it appears; `Redacted` is a wrapper named for what was done to its contents. `From*` builds `Self` from a source and `Into*` consumes `self`; `Try*` appears only beside an infallible sibling, so a fallible method with none returns `Result` under the plain name.
4. **The core has no runtime.** It uses `std::future`, a `BoxFuture` alias, and two pluggable traits (§3.9): `Timer`, the clock, and `Runtime: Timer + Spawn`, the clock with an executor, which an app that binds a transport is given and every transport spawns through. Runtime crates (such as `fw-runtime-tokio`) implement them and bring the signals; transports bring the sockets, the hyper servers through a listener crate per runtime (transports §3.7). Independence from a runtime is stated as two levels, each read off `cargo tree`, and CI's `runtime levels` job holds every workspace member to the one `ci/runtime-levels.txt` gives it (transports §8). The core is *runtime-free*: no `tokio` crate anywhere in its dependency tree, the level the transport hubs, their plug-point traits and types and the second runtime crate, `fw-runtime-smol`, hold too, so a plug point is implemented without tokio. The hyper servers are *runtime-neutral*: they run entirely on the runtime the app is given and carry tokio's `sync`, and on the HTTP/2 path `io-util` and `bytes`, through hyper and `h2`, none of which is a runtime. A plug point inherits the level of the layer it plugs into: the hyper servers' `Listener` and `Incoming` are runtime-neutral as the layer is, and a plug point at either level is implemented without a tokio runtime (transports 115).
5. **Identity is by type.** Keys are `TypeId`s, so a type alias or a renamed import reads the same binding [9][16].
6. **`unsafe` is refused everywhere but where it is listed.** The workspace denies `unsafe_code` through `[workspace.lints.rust]`, every member inherits the table with `[lints] workspace = true`, and a module whose `unsafe` has no safe equivalent opts out with `#![allow(unsafe_code)]` at its top and an entry in `ci/unsafe-allowed.txt`, each entry marked `pure-rust`, `unsafe` Miri executes, or `syscall` with the reason it cannot; `scripts/unsafe_allowed.py check` fails CI on `unsafe` or a relaxed `unsafe_code` anywhere else and on an entry with no kind, and the `miri` job runs Miri over each pure-Rust entry's tests, failing when it runs none, and prints each system-call entry it skips with the reason (transports §0). The core has no `unsafe` and is not listed.
7. **A public signature names only what its crate reaches.** Every type a crate's public signatures name is reachable from that crate, through a public path or a re-export, so a user writes a type a handler takes or returns without adding a dependency no documentation names; `std`, `core`, `alloc`, serde and the core are exempt, every application depending on the core and every serde trait arriving with the derive that implements it. A transport crate re-exports `fw-transport`'s `CallError`, `Classify`, `Detail`, `Details`, `ErrorKind`, `ExtractError`, `FieldViolation`, `FromCall`, `IntoReply`, `IntoReplyError`, `Valid` and `Validate`, the three detail types because the re-exported traits' and `CallError`'s own signatures name them, `Link`, which `Detail::Help` names, as `details::Link` because `fw_rpc` has a `Link` trait at its root, and its builders' `Bound` and `Count` under `config`, as `fw_http::config` does, and `serde_json`'s `Value`, which `Detail::Json` carries, under `json`. A type from another crate that a user names, to build a value, match on it or call a method, is re-exported by the crate whose signature names it, the specific type and never the whole third-party crate, whose semver would then be the crate's: `serde_json`'s types under a `json` module wherever a signature names them, `tonic_health::ServingStatus` from `fw-grpc`, `fw-config`'s `Problems` from `fw-db` for a store's `validate`, `Env` and `Problems` under `fw_otel::config` for `OtelOptions::from_env`, and the layer `Otel::layer` returns with its tracer. `scripts/public_reach.py` checks the rule in CI's `public reach` job over the crates `ci/public-reach.txt` lists, reading each crate's rustdoc JSON, and fails on a finding the file does not allow and on an allowance nothing matches; each allowance is a case left as it stands, with its reason: a type no user names (a trait the framework calls, the type behind an alias the user writes), or one whose dependency the user holds whatever is re-exported (a `Config` built through `fw-config`'s builder, a span a transport instruments with `tracing::Instrument`, a stream read through `futures`'s extension traits). `#[derive(Classify)]` and `#[derive(Validate)]` expand to paths through the first of `fw-transport`, `fw-rpc`, `fw-http`, `fw-ws` and `fw-grpc` the deriving crate depends on, each of which re-exports what the generated code calls, so a crate deriving either needs no dependency beyond its transport.
8. **A second spelling stays only if it lowers to the first or says something the first cannot.** The documentation promotes one spelling for each situation. Between two spellings that say the same thing, the one kept is the one the compiler checks more, that needs fewer imports, and whose errors point at the user's code; review enforces the rule. Struct-field injection and constructor injection both stay, the constructor form being able to be async and fallible, and the documentation leads with the constructor; `new` and `#[construct]` both stay, `new` the documented spelling; a bare closure in a `providers` list stays as shorthand for `with = closure`, to which it lowers (§4); `ModuleDef::meta::<T>()` lowers to `meta_with(T::default)` (§13).

---

## 1. Crate layout

| Crate | Contents |
|---|---|
| `fw-core` | Keys, bindings, injection points, scopes, executions, modules, lifecycle, errors, and enhancer traits generic over a transport |
| `fw-macros` | `#[injectable]`, `#[controller]`, `#[construct]`, `#[module]`, `#[handlers]` |
| `fw-http`, `fw-ws`, `fw-rpc`, `fw-grpc` | Transport marker types, contexts, handler attributes, middleware, servers |
| `fw-runtime-tokio` (and others) | `Runtime` and `Timer` implementations, OS signal future, spawn helpers |
| Integrations (`fw-sqlx`, `fw-redis`, `fw-config`, ...) | Ordinary modules written against the value API |

---

## 2. The model at a glance

- **Key** is the pair of a type and a qualifier: `Key::of::<PgPool, Replica>()`. The qualifier defaults to `()`.
- **Binding** holds a key, a kind (single or contribution), a declared scope (`Auto` where none is written, §3.3), a recipe (construct, value, factory, alias), its declared injection points, its origin module, and its source location (captured with `#[track_caller]`).
- **Injection point** is a field or parameter the container fills. Its type implements `FromContainer`: it describes the key(s) it reads and knows how to read itself. `Dep<T, Q>`, `Many<T, Q>`, `Ext<T>`, `Option<S>`, `ModuleRef`, `ExecutionRef`, plus any type an integration crate defines.
- **Module** is a Rust type with an identity. It declares imports, bindings, controllers and exports.
- **Graph** is the frozen, validated result of registering every module.
- **App** is the graph plus the singleton store. It moves through typestates: `Wired` → `Connected` → `Bound` → serving → closed. From `Connected` on it hands out an `AppHandle`, a `Clone + Send + Sync` view of the shared state that outlives `serve`.
- **Execution** is one call. It holds a per-execution cache, an extension bag, execution inputs (the request or call context), a cancellation signal, and an optional deadline. The framework or the transport holds it; everything else holds a cheap-clone handle to it. Cancellation fires while handles are alive: for a call when the client disconnects or its deadline passes, for a standalone execution at its deadline, and for either at the end of the drain (§9.5). The execution ends when its holder drops it, whatever handles remain.

The lifecycle runs like this:

```
register modules (sync)
   → wire: build graph + validate everything      [no instances, no I/O]
   → connect: singletons in dependency order,
              readiness checks, init hooks, bootstrap hooks
   → bind: transports bind sockets
   → serve: each call opens an Execution
   → close(signal), or the signal passed to serve, whichever comes first:
              before-shutdown → stop accepting → drain → cancel + abandon the rest
              → destroy → sockets closed → shutdown
```

---

## 3. Core types and traits

### 3.1 Keys and qualifiers

```rust
/// A type as a report names it: `TypeName::of::<my_app::User>()` prints `User`, and with `{:#}`
/// prints `my_app::User`. Two names compare and hash on their `TypeId`s, so a type alias or a
/// renamed import names the same type; the `type_name` string is how the name prints and nothing else.
#[derive(Clone, Copy)]
pub struct TypeName { id: TypeId, name: &'static str }

impl TypeName {
    pub fn of<T: ?Sized + 'static>() -> TypeName;
    /// The names among `names` whose short form another, different type among them shares,
    /// `a::Config` beside `b::Config`: the ones a report writes with `{:#}` (§10.1).
    pub fn colliding(names: impl IntoIterator<Item = TypeName>) -> HashSet<TypeName>;
}
// Display: "User", "Arc<PgPool>"; `{:#}`: "my_app::User", "alloc::sync::Arc<my_app::db::PgPool>"

#[derive(Clone, Copy)]
pub struct Key { ty: TypeName, qualifier: TypeName }
// PartialEq, Eq and Hash are written by hand over the two names, which compare on their
// TypeIds. `type_name` is not a const fn, so each name is read at `of`.

impl Key {
    pub fn of<T: ?Sized + 'static, Q: 'static>() -> Key;
    pub fn type_name(&self) -> TypeName;   // the type the key binds, without its qualifier
}
// Display: "PgPool", "PgPool @ Replica"; `{:#}`: "my_app::db::PgPool @ my_app::Replica". A report
// decides full paths per `TypeName`, not per key: `Store @ a::Replica` beside `Store @ b::Replica`.
// `KeyName` is a `Key` with its `BindingKind`, "dyn Plugin (collection)", the form the errors carry.
```

A qualifier is any `'static` type, usually a unit struct: `pub struct Replica;`. Because keys are types, there are no strings to misspell [9]. A `Key` is also a runtime value: an integration crate that holds one can look it up erased through `Resolver::by_key::<T>(key)`, which is the one lookup that can ask for a `T` the key does not hold (§10.2). A `TypeName` is what a report holds for a type that is never bound, a transport's marker type or the `T` an extractor reads; a `Key` is a binding's identity.

### 3.2 Injection points

```rust
pub struct Dep<T: ?Sized, Q = ()> {
    inner: Arc<T>,
    _q: PhantomData<fn() -> Q>,
}
// Deref<Target = T>, Clone, Dep::from_arc, Dep::into_arc, Dep::arc; Debug writes the key, `Dep<PgPool @ Replica>`

pub struct Many<T: ?Sized, Q = ()> {
    items: Arc<[Arc<T>]>,
    _q: PhantomData<fn() -> Q>,
}
// Deref<Target = [Arc<T>]>, iter(); Debug writes the key, `Many<dyn Plugin>`

pub struct Ext<T>(Arc<T>);   // a typed view of per-execution data [12]; Debug writes `Ext<T>`
pub struct ModuleRef { /* graph + module id + optional execution */ }
pub struct ExecutionRef { /* a cheap-clone handle to the current execution (§3.8) */ }
```

`FromContainer` is how every injection point is read. Fields, constructor parameters, factory parameters and closure parameters all go through it, which is what gives them one rule [11]. The name follows the `From*` rule (§0): the type is built from the container, as a `FromStr` type is built from a string, and the diagnostic says what to write instead, so nobody implements it for `PgPool` themselves. The transport layer's `FromCall` is its pair, a parameter built from the call, and the two names tell a reader of a diagnostic which source a parameter comes from.

```rust
#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot be obtained from the container",
    label = "the container cannot provide this",
    note = "write `Dep<{Self}>` to share the container's instance",
    note = "or set it inside the #[construct] fn / mark the field #[construct(default)]"
)]
pub trait FromContainer: Sized + Send + 'static {
    /// What this type reads, for the wiring pass: key, kind, optional, needs-execution.
    fn describe(req: &mut Requirement);

    /// Build the value from the container.
    fn from_container(r: &Resolver<'_>) -> impl Future<Output = Result<Self, LookupError>> + Send;
}

/// What one injection point reads. `describe` fills it: `dep(key)`, `many(key)`, `ext::<T>()`,
/// `execution()`, `module()`, and `optional::<S>()` for everything `S` reads, marked optional.
/// `dep_imported(key, import)` reads `key` as one a module binds once the application imports it,
/// `import` written as the import is, and a key no module binds is then `WiringError::NotImported`
/// naming the import rather than `Missing` (§10.1).
pub struct Requirement { /* reads */ }

/// The declared injection points of a binding or a closure, one `Requirement` each, which the
/// wiring pass resolves. `field` and `param` carry a name for diagnostics; `add` is the unnamed
/// form, for a closure parameter, named by its position.
pub struct Dependencies { /* records */ }
impl Dependencies {
    pub fn field<S: FromContainer>(&mut self, name: &'static str) -> &mut Self;
    pub fn param<S: FromContainer>(&mut self, name: &'static str) -> &mut Self;
    pub fn add<S: FromContainer>(&mut self) -> &mut Self;
}
```

Implementations in the core:

| Type | Reads | Notes |
|---|---|---|
| `Dep<T, Q>` | single binding `T @ Q` | the shared `Arc` |
| `Many<T, Q>` | every contribution to `T @ Q` | collection order; eager |
| `Option<S>` | `S`, or `None` where `S` would fail with `NotFound` | optional deps [13] |
| `Ext<T>` | extension `T` of the current execution | needs an execution |
| `ModuleRef` | handle to the enclosing module [37] | |
| `ExecutionRef` | a handle to the current execution | needs an execution |

**Collection order** is one rule for every reader of a collection: depth-first post-order over imports from the root, imports in the order written, then declaration order inside a module. A module's contributions follow those of everything it imports, and two modules with no import edge between them are ordered by where the walk reaches them. An import that closes a cycle leads to a module the walk has already reached and is not followed again, so each module of a cycle is placed once, after the modules it imports that the walk had not yet reached.

`Many<T>` is the eager form: reading it constructs every contribution. Transports read a role collection lazily through `Resolver::entries::<T>()`, which yields one handle per contribution in collection order, each with its own `resolve().await`; §7's pipeline is written against it.

A hand-written read goes through the `Resolver`: `r.dep::<PgPool>()` reads `T @ ()`, and a qualifier is a second method, `r.dep_qualified::<PgPool, Replica>()`, since a function's type parameters take no defaults and a `Q` on `dep` would break the plain spelling. `many` and `many_qualified` pair the same way; `entries` takes no qualifier, as role collections are unqualified.

`Option<S>` is `None` exactly when `S` would fail with `LookupError::NotFound`: a key no module binds, an extension no guard has written, an input this execution did not seed. Every other error propagates: a construction failure, `ExecutionRequired`, an ambiguous key or module.

Execution inputs, meaning the request or call context, are ordinary `Dep<T>` injection points. The transport declares the key as an *input* (§6.4). Integration crates can add their own `FromContainer` types, for example `fw-ws` defines `Session<T>` for per-connection state.

### 3.3 Scopes

```rust
pub mod scope {
    pub struct Singleton;
    pub struct PerExecution;
    pub struct Transient;
    pub struct Auto;   // what #[injectable] means when no scope is written
}

pub trait Scope: sealed::Sealed + 'static {
    const KIND: ScopeKind;
}

#[diagnostic::on_unimplemented(
    message = "lifecycle hooks are only allowed on singletons",
    label = "`{Self}` scope cannot have hooks",
    note = "remove the scope argument from #[injectable] or #[controller], or move the hook to a singleton"
)]
pub trait HookCapable: Scope {}          // Singleton, Auto

#[diagnostic::on_unimplemented(
    message = "a `{S}` type cannot read `{Self}`",
    label = "this injection point needs an execution",
    note = "declare the type #[injectable(execution)] or #[injectable(transient)]; a controller drops `singleton` from #[controller] and is then built per call"
)]
pub trait AllowedIn<S: Scope> {}         // implemented per injection-point type and scope
// `{Self}` is the injection point's type; the type that cannot read it is the one declared `{S}`.
```

What `Auto` means depends on the binding's role:
- For a **provider**, `Auto` behaves exactly like `Singleton`. Depending on execution data is refused [21].
- For a **controller or enhancer**, `Auto` resolves to singleton if nothing below it needs an execution, and to per-execution otherwise [22][24][26].

Writing `#[controller(singleton)]` explicitly opts out of this inference, so a controller declared that way that needs execution data is refused, the report's help reading "remove `singleton` (an `Auto` controller is built per call when it needs to be), or inject a factory.", with `#[controller(execution)]` as a third option on the line after. A controller is a type implementing `Controller`, the mark `#[controller]` writes beside the `Construct` impl `#[injectable]` writes, and `Handlers`, whose `mount` declares its handlers; a module lists a controller under `controllers` and nowhere else, so a type's `Auto` is read in the role its declaration names (§14, item 28).

Scope is one axis and construction is another. How a binding is built is a type, a value (`value = expr`) or a closure (`with = closure`), and that is all the spelling says; a value is built once where it is written and has no scope to choose. How long a type's or a closure's instance lives is its scope: `Auto` unless one is written, with the two meanings above. The same closure means the same thing on a method, on a controller impl, in an `into` list, in a `providers` list and through the value API. One override serves every position, mirroring `#[injectable(singleton | execution | transient)]` on a type:

| Where | `Auto` | Explicit |
|---|---|---|
| `#[guards]`, `#[interceptors]`, `#[error_handlers]` (§7) | `with = closure` | `with(singleton \| execution \| transient) = closure` |
| an `into K: [..]` list (§4) | `with = closure` | the same three |
| a `providers` list (§4) | `with = closure`, the bare closure, or `K: with = closure` under a trait key | the same three |
| `ModuleDef` and `Contribute` (§13) | `with` / `try_with` | `singleton` / `execution` / `transient` and their `try_` forms |
| `EnhancerSpec` (§13) | `guard_with(f)`, `interceptor_with(f)`, `error_handler_with(f)` | `guard_with_in::<S>(f)`, `interceptor_with_in::<S>(f)`, `error_handler_with_in::<S>(f)`, `S` one of the markers above |

An explicit `singleton` that needs an execution is refused at `wire()`, type or closure alike [21] (§6.2).

Inference reads what a closure reads, not what it does. `|| RequestTimer::start()` reads nothing, so `Auto` builds it once and every call shares the one timer. A closure that creates per-call state writes `with(execution)`: that is the case the explicit form exists for, and the one inference cannot see.

### 3.4 Construct

```rust
pub trait Construct: Sized + Send + Sync + 'static {
    type Scope: Scope;

    /// How long `construct` may take (§3.9). `#[injectable(timeout = ..)]` writes `After`.
    const CONSTRUCT_TIMEOUT: Bound = Bound::Default;

    /// Declared injection points, used by the wiring pass. Generated from the fields or ctor params.
    fn dependencies(d: &mut Dependencies);

    /// Build the instance. Async and fallible [15].
    fn construct(r: &Resolver<'_>)
        -> impl Future<Output = Result<Self, ConstructError>> + Send;

    /// Lifecycle hooks this type implements. The macro fills this in by probing;
    /// a hand-written impl calls the `Hooks<Self>` methods it implements (§9.1).
    fn hooks(_h: &mut Hooks<Self>) {}
}
```

`Construct` is not generic over the resolver. `dependencies()` is the single source of truth for what a type reads.

A read in `construct` outside what `dependencies()` declared is refused, in every build, at the type's first construction. The resolver `construct` is handed checks each `dep`, `many`, `entries`, `input`, `execution` and `by_key` against the keys `dependencies()` described, and `ext` against the extension types it declared (wiring reads only that an `Ext<T>` needs an execution, the check also compares `T`, so declaring `Ext<A>` does not cover a read of `Ext<B>`): an `Option<Dep<T>>` declares `T` as a `Dep<T>` does, a `Many<dyn K>` declares the collection, and a qualified `Dep<T, Q>` declares `T @ Q` alone, not `T`. The refusal is `Errored` holding a `Permanent` `UndeclaredRead` that names the key and the reading type, and it fails the construction whatever the body does with the read's error: `ConnectError::Construct` during `connect`, `LookupError::Construct` inside a call, where an execution-scoped or transient type is refused at each construction. A generated impl describes what it reads and never meets it. A `ModuleRef` or `ExecutionRef` reads without a declaration, inside `construct` or kept past it, because the check covers only the resolver a `construct` runs against; its reads are runtime lookups. The check is a scan of the declared keys, a handful of `Copy` comparisons per read. A hand-written `Construct` states each dependency twice, once in `dependencies` and once in `construct`, and this check is what keeps the two lists equal.

The code `#[injectable]` writes into `dependencies` and `hooks` sits in the user's module, as every attribute's output does, so a user trait in scope with a method every type has is a candidate for each call it makes by method syntax. Every autoref probe the attributes emit therefore names its method with a double-underscore framework prefix (`__ulo_field`, `__ulo_register_init`, `__ulo_check`), and an inherent `&mut self` method called on an owned value is called by path, since a user's `&self` method is reached one autoref step before it. A probe sharing a name with a user's method is ambiguous (E0034) where its arm applies and the user's method where it does not, which for a check returning `()` is a refusal that never fires.

`CONSTRUCT_TIMEOUT` bounds `construct` wherever the instance is built: during `connect` for a singleton, and inside the call for an execution-scoped or transient binding, where a hanging constructor would block the call. `Default` is the app's `construct_timeout`, 30 s. Expiry reports as `FailureReason::TimedOut`, and a bound the `Timer` refuses as `Errored` holding a `Permanent` `RuntimeShutDown` (§3.9), on `ConnectError::Construct` during `connect` and on `LookupError::Construct` inside a call (§10.2). A factory takes the same bound through `.timeout(..)` on its binding handle (§9.1).

`ConstructError` (§10.2) separates a dependency's failure from the constructor's own. `Dependency(LookupError)` is a dependency read that failed: an input the execution did not seed, `ExecutionRequired`, a nested construction. The core passes it through unchanged, and what the caller gets is the dependency's own error naming the deeper key; an error naming the outer constructor would point at the wrong place. `Failed(BoxError)` is the constructor's own error, the one reported as `Construct { reason: Errored(..) }`. `From<LookupError>` is implemented and no blanket `From<E: Error>` is: `LookupError` implements `Error`, and the two impls would overlap (E0119). `?` on a dependency read is written as is, and the constructor's own error goes through `ConstructError::failed(e)`. `#[injectable]` writes that mapping for a `new` or `#[construct]` fn returning `Result<Self, E>`; a hand-written impl writes `.map_err(ConstructError::failed)?` (§13).

### 3.5 Lifecycle traits

```rust
pub trait OnModuleInit: Construct<Scope: HookCapable> {
    const TIMEOUT: Bound = Bound::Default;
    fn on_module_init(&self) -> impl Future<Output = Result<(), BoxError>> + Send;
}
pub trait OnApplicationBootstrap: Construct<Scope: HookCapable> {
    const TIMEOUT: Bound = Bound::Default;
    fn on_application_bootstrap(&self) -> impl Future<Output = Result<(), BoxError>> + Send;
}
pub trait OnModuleDestroy: Construct<Scope: HookCapable> {
    const TIMEOUT: Bound = Bound::Default;
    fn on_module_destroy(&self) -> impl Future<Output = ()> + Send;
}
pub trait BeforeApplicationShutdown: Construct<Scope: HookCapable> {
    const TIMEOUT: Bound = Bound::Default;
    fn before_application_shutdown(&self, signal: &Signal) -> impl Future<Output = ()> + Send;
}
pub trait OnApplicationShutdown: Construct<Scope: HookCapable> {
    const TIMEOUT: Bound = Bound::Default;
    fn on_application_shutdown(&self, signal: &Signal) -> impl Future<Output = ()> + Send;
}
```

`TIMEOUT` bounds the hook (§3.9): `Default` is the app's `hook_timeout`, 10 s, `After(d)` an explicit bound, `Unbounded` none. Each trait carries its own const, so a type implementing two hooks bounds each separately (§9.1).

The `Construct<Scope: HookCapable>` supertrait makes a hook on an explicitly execution-scoped or transient type a compile error [41]. `#[injectable]` fills in `Construct::hooks` using autoref probing (Self is concrete inside the generated impl), so implementing a hook trait is all a user does. No marker attribute is involved.

### 3.6 Module

```rust
pub trait Module: Send + Sync + 'static {
    /// Identity for deduplication and diagnostics [31][35].
    fn identity(&self) -> ModuleIdentity;

    /// Declare everything. Synchronous and free of I/O.
    fn register(&self, m: &mut ModuleDef<'_>);

    /// What the module is configured with outside its identity, compared across imports (§8.3).
    fn settings(&self) -> ModuleSettings { ModuleSettings::new() }
}

impl ModuleSettings {
    pub fn new() -> Self;
    pub fn with(self, name: &'static str, value: impl Debug) -> Self;  // the value as `Debug` writes it
}

impl ModuleIdentity {
    pub fn of_type<M: 'static>() -> Self;                                   // unit modules
    pub fn of_value<M: Eq + Hash + Clone + Send + Sync + 'static>(m: &M) -> Self; // configured modules
    pub fn of_owner<O: 'static, C: Eq + Hash + Clone + Send + Sync + 'static>(config: &C) -> Self; // type O, configuration C
    pub fn label(self, name: &'static str) -> Self;                          // readable name override
}
```

`register` returns `()` and never returns early. A value that may fail to build goes through `m.try_value(result)`, which records an `Err` on the `ModuleDef` for `wire()` to report beside every other wiring error, under a `WiringErrors` entry naming the module and the key; a contribution's goes through `Contribute::try_value(result)` (§4), recorded the same way under the collection key. Stopping at the first failure would hide the rest, which is what [44] rules out.

Identity is the module's `TypeId`, plus its configuration value if it has one, plus a qualifier if it's keyed. Two configurations of one type are two modules. The same configuration imported twice is one module [31], and both imports must carry the same settings (§8.3).

Diagnostic names are the type name, the key and an optional label, as in `DbModule @ Replica`. The second and later module of one type and qualifier, in collection order, carries `#n`, labelled or not: `DbModule #2`. A label names every configuration of an integration alike, and two modules printed alike cannot be told apart. The configuration's `Debug` output is never printed, because it may hold credentials.

### 3.7 Transports and enhancer roles

```rust
/// Implemented by marker types in transport crates: fw_http::Http, fw_rpc::Rpc, ...
pub trait Transport: 'static {
    type Cx: Clone + Send + Sync;   // per-call context: a cheap-clone handle to the execution
    type Reply: Send;
}

pub trait Guard<T: Transport>: Send + Sync + 'static {
    fn can_activate(&self, cx: &T::Cx)
        -> impl Future<Output = Result<bool, BoxError>> + Send;
}
pub trait Interceptor<T: Transport>: Send + Sync + 'static {
    fn intercept(&self, cx: &T::Cx, next: Next<'_, T>)
        -> impl Future<Output = Result<T::Reply, BoxError>> + Send;
}
pub trait ErrorHandler<T: Transport>: Send + Sync + 'static {
    fn handle(&self, err: BoxError, cx: &T::Cx)
        -> impl Future<Output = Result<T::Reply, BoxError>> + Send;
}

// dyn-compatible twins with blanket impls, used as role keys
pub type AnyGuard<T> = dyn ErasedGuard<T>;
pub type AnyInterceptor<T> = dyn ErasedInterceptor<T>;
pub type AnyErrorHandler<T> = dyn ErasedErrorHandler<T>;

/// The role keys. Sealed: `AnyGuard<T>`, `AnyInterceptor<T>` and `AnyErrorHandler<T>` implement it
/// for every `T: Transport`, and nothing else does.
pub trait Role: sealed::Sealed + Send + Sync + 'static {}
```

`Guard<Http>` and `Guard<Rpc>` are different traits, so an HTTP guard and an RPC guard are different roles [25]. A role comes only from a trait implementation. Which contributions are enhancers is decided by `TypeId` at freeze: mounting a handler records the three role keys of its transport, `AnyGuard<T>`, `AnyInterceptor<T>` and `AnyErrorHandler<T>`, and every contribution whose key is one of them is an enhancer, however it was registered (§7). `ModuleDef::enhancer::<R: Role>()` marks a contribution explicitly, for a role key of a transport with no mounted handler, and its bound refuses a type that is not a role key at compile time. Detection reads no type name and survives a compiler changing `type_name`'s format.

`Cx` wraps an `ExecutionRef` together with the call's wire data, and every clone is the same call. Everything a guard or interceptor writes goes through a shared reference and interior mutability, which the extension bag already uses. A reply that streams carries a clone of `Cx`, which is how a body, a reply stream or a WebSocket item sequence reads its execution and observes its cancellation past `intercept`'s return; its per-execution instances stay alive because the transport holds the `Execution` until the reply is written (§3.8). A request body stream lives on `Cx` behind a take-once slot, not as an execution input: inputs are `Sync` (§6.4) and a body stream is read once.

### 3.8 Execution

```rust
pub struct Execution { /* Arc: cache, extensions, inputs, cancel, deadline, module */ }   // not Clone
pub struct ExecutionRef { /* a clone of that Arc */ }   // Clone + Send + Sync

impl Execution {
    pub fn extensions(&self) -> &Extensions;               // typed bag [23]
    pub fn cancelled(&self) -> Cancelled<'_>;              // future, runtime-free
    pub fn is_cancelled(&self) -> bool;
    pub fn draining(&self) -> Draining<'_>;                // future: resolves as the drain begins (§9.5)
    pub fn is_draining(&self) -> bool;
    pub fn deadline(&self) -> Option<Instant>;
    pub fn handle(&self) -> ExecutionRef;                  // a clone, for a guard, a reply stream or a subtask
    pub fn seed<T: Send + Sync + 'static>(&self, input: T);
    pub async fn get<T: ?Sized + Send + Sync + 'static>(&self) -> Result<Dep<T>, LookupError>;
}
// ExecutionRef has the same methods except `seed`, and its two futures are owned:
impl ExecutionRef {
    pub fn cancelled(&self) -> Cancelled<'static>;         // borrows nothing from the handle
    pub fn draining(&self) -> Draining<'static>;           // `stream.take_until(exec.draining())`
}
```

`Execution: Send + Sync`, and `Resolver<'_>` is `Sync` with it. That is what lets `FromContainer::from_container` and `Construct::construct` hold the resolver across an await and still return `Send` futures; a `!Sync` value anywhere in the execution would make every read's future `!Send`, reported at the trait's `+ Send` rather than at the value. `seed` is bounded `Send + Sync` for the same reason.

`seed` is on `Execution` alone, the holder's handle. A guard or a subtask holds an `ExecutionRef`, and an input seeded there would be one the wiring pass never saw. `Execution` is not `Clone`; a second holder takes `handle()`.

Who holds the execution: the transport for a call, `execute` for a standalone one. Everything else holds a clone, `ExecutionRef` directly or inside a `Cx`. Three events are distinct:
- **Draining** is the notice. `draining()` resolves and `is_draining()` turns true as the drain begins (§9.5), a moment before the servers stop accepting: the core fires it, then hands each server its `DrainToken`, and a server's drain raises its own signal that its accept loop reads on its own task, so a connection can still be accepted in that moment; the phase is already Draining, so its request is refused as the drain refuses one (503 with `Connection: close` on HTTP), not served. Both are false through the before-shutdown stage, where traffic is normal. A streaming reply that sees it finishes what it has and ends; the synchronous form is for a loop that checks between items rather than racing a future.
- **Cancellation** fires while clones are alive, so a streaming reply observes it. The transport fires it for a call when the client disconnects or the deadline passes, a standalone execution's fires at its deadline, and the drain's end fires it on every execution still alive, transport-opened and standalone alike (§9.5). The drain's end is `drain_timeout` or, earlier, the moment the `shutdown_timeout` cap expires: the same source, not a fourth. Completion of `execute` does not fire it, as finishing a response does not cancel a streaming body. A subtask that should stop when the job finishes is awaited by the job, not detached.
- **End of execution** is when the `Execution` drops, whatever clones remain. The per-execution cache is released then and execution-scoped instances are dropped, so an execution-scoped service holding its `ExecutionRef`, §5's `AuditContext`, is no cycle. Every transport holds the `Execution` until the reply is written, a streaming body included, so a streaming reply keeps its instances exactly as long as it runs. A clone kept past the end, by a detached subtask, still answers what the execution recorded, `deadline`, `is_cancelled`, `cancel_reason`, `extensions` and `trace` among them; its `get` answers `LookupError::ExecutionEnded` whatever the key's scope, rather than building an instance for an execution that is over, and a build already running at the end completes for its readers and is not kept. A subtask that should finish within the execution is awaited by its holder. Each instance drops under `catch_unwind`: one whose `Drop` panics does not stop the others from dropping or the execution from ending, and the first panic is logged at `warn`, redacted (§3.9).

`get` resolves against the visibility of the execution's module: the dispatching controller's module inside a transport call, the root module in a standalone execution opened on the app, or the module whose `execute` opened it (§8.2).

The execution cache uses a runtime-agnostic async once-cell (for example from `async-lock`, which is not a runtime). If two injection points in one execution resolve the same key concurrently, they get one instance. A build that fails is kept for the execution as an instance is: readers waiting on it share the one attempt, a later reader in the same execution receives the same `LookupError` without a build, and the next execution builds again. Every reader's error holds the one original behind a shared pointer, which `Redacted::downcast_ref` and `Redacted::original` reach from each of them. A construction with side effects therefore runs once per execution whether it succeeds or fails (§14, item 32). The kept failure holds for state written later in the same execution too: an execution-scoped binding whose construction reads an `Ext` before a middleware has inserted it fails, and every later reader in that execution receives that failure after the middleware has run. A binding built from such state is read where the state is already written, or reads it at use through an `ExecutionRef` rather than at construction. The enhancer closures the app builds once keep no failure: a failure kept there would last as long as the app, and the next call builds again. `Cancelled` and `Draining` are implemented in the core with a waker list. The core stores the deadline; the transport's runtime enforces it for a call, and `execute` enforces a standalone one on the app's `Timer` (§6.3).

### 3.9 Timer, runtime and bounds

```rust
pub trait Timer: Send + Sync + 'static {
    fn sleep(&self, d: Duration) -> BoxFuture<'static, ()>;
    fn try_sleep(&self, d: Duration) -> BoxFuture<'static, Result<(), Refused>>;
    fn now(&self) -> Instant;
}

pub trait Spawn: Send + Sync + 'static {
    fn spawn(&self, fut: BoxFuture<'static, ()>) -> TaskHandle;
    fn try_spawn(&self, fut: BoxFuture<'static, ()>) -> Result<TaskHandle, Refused>;
}

pub trait Runtime: Timer + Spawn {}      // implemented for every `Timer + Spawn`

pub enum Bound {
    Default,            // the app's default for this kind of bound
    After(Duration),
    Unbounded,
}

pub enum Cut { Expired, Refused(Refused) }
pub struct Expiry<'t> { /* .. */ }     // Future<Output = Cut>: `Expiry::at(timer, until)`, `Expiry::after(timer, d)`; `passed()`, the second tie rule
pub async fn timeout<F: Future>(timer: &dyn Timer, d: Duration, fut: F) -> Result<F::Output, Cut>;
pub async fn timeout_at<F: Future>(timer: &dyn Timer, until: Instant, fut: F) -> Result<F::Output, Cut>;
```

The core has no clock of its own. Every wait it times, the drain, a hook, a construction, a readiness check, a transport's `close` at shutdown, runs through the app's `Timer`, set on the builder (§9.4). `sleep` returns a boxed `'static` future: the trait stays dyn-compatible, which `-> impl Future` is not, and a caller can spawn what it returns. `timeout(timer, d, fut)` races `fut` against an `Expiry`, a wait on `try_sleep` that answers how it ended, and polls `fut` first; a wait whose end only ends work races `sleep` itself. `now` sits on the same trait because deadlines and sleeps have to read one clock; measured with `std::time::Instant`, a deadline on a paused test clock never arrives while sleeps keep resolving.

A runtime that has shut down refuses as a value and never panics. `try_sleep` answers `Err(Refused)` and `try_spawn` answers `Err(Refused)`, meaning the task was not started or a shutdown stopped it before it finished, so a refusal is no proof that none of its work ran; `sleep` ends at once, a handle `spawn` or `spawn_blocking` answers ends `Aborted` with nothing run, and `now` still answers. `try_sleep`, `try_spawn` and `spawn_blocking` are required (transports 333), so a wrapper forwards them and cannot drop a refusal by omission. A runtime with no shutdown, `fw-runtime-smol`'s, writes all three and never refuses. Which form a wait uses follows from what it is for:

| A sleep that… | Uses | On refusal |
| --- | --- | --- |
| **bounds** work raced against it, its expiry only ending the work | `sleep` | an early end is a correct "too late" |
| **bounds** work raced against it, its expiry reported to a reader | `timeout`, an `Expiry` on `try_sleep` | the refusal is reported as one |
| **paces** a loop (backoff, retry, poll interval) | `try_sleep` | the loop ends |
| **holds** something open (a grace) | `try_sleep` | the thing is released at once, logged at `debug` |

A pacing loop on plain `sleep` turns "ends at once" into a busy loop. A bound whose expiry is reported as a reason someone reads, a close code, an error a deployer sees or a hook's outcome, reports a refusal as a refusal, through `timeout` or an `Expiry`. A bound whose expiry only ends work runs on `sleep`, and an early end reads as "too late".

The ties between a refusal and what else is ready live in `Expiry` alone, and every reporting bound, the core's and the integrations', goes through it. In order:
1. An answer ready in the same poll wins. A refusal is answered one poll late, so the work raced against it is polled once more first.
2. A refusal that arrives once the clock reads the bound as passed reports the expiry: the bound did pass.
3. Otherwise the refusal.

The core's own bounds report through it: a hook's, a construction's, a readiness check's, the shutdown cap and a transport's `close` bound. A refused bound ends its item at once, as an expired one does, and the item reports `FailureReason::Errored` holding a `Permanent` `RuntimeShutDown` (§10.2): one type means "the runtime refused" at every site, so a deployer matching on it finds the hook, the constructor, the readiness check and an integration's wait in one arm. A refused shutdown cap acts as a spent cap: the item it bounds is cut at once, a `close` still getting its first poll, and nothing is left unbounded. Outside the core, the RPC client's per-call bound on a dead handed runtime answers `Unavailable` with the reason `runtime_shut_down`, and `Timeout` when its deadline had passed before the refusal; the graphql-ws `connection_init` bound closes 1001 with the shutdown phrase, and 4408 when the init was due before the refusal; the WebSocket ping tick and wait for a Pong end the connection `RuntimeShutDown`, and `Lost` when the Pong was due; NATS's roots-load bound and `ulo-db`'s `connect_within`, migration-lock wait and `migrate_within` report `runtime_shut_down`, as a `Permanent` startup error where the start fails. The drain window, the per-call deadlines, the handshake timeout and the health check's bound only end work, and stay on `sleep`.

The second rule reaches a refusal of another wait inside the bound too. A loop that reports the refusal of a pacing wait, or of a wait beside the bound, first asks whether the bound it runs under has passed, `Expiry::passed` or the clock against the bound's end, and reports the expiry if it has: answered in the poll the expiry fires in, the refusal would otherwise outrank it under the first rule. The readiness backoff under the whole-check bound, `ulo-db`'s create backoff under `connect_within` and the WebSocket ping tick beside an outstanding Pong do, and so does Kafka's coordinator retry in `anchor`, which waits only while its next backoff still fits before the deadline (transports §5.3, 30).

A `Runtime` is a `Timer` with an executor. `Spawn::spawn` starts a `BoxFuture` as a task and answers a `TaskHandle`, which detaches on drop, stops the task on `abort`, and awaited answers how the task ended; the core spawns nothing of its own and holds the runtime for the transports, every one of which spawns its connections and calls through it, so `listen()` refuses an app that binds a transport without one (§9.5), while an app that binds nothing needs the clock alone. The runtime is set on the builder with `.runtime(r)` (§9.4); `fw-runtime-tokio`'s `Tokio` is one. A task that panics ends as `TaskEnd::Panicked(Arc<Redacted>)`, its message converted as `PanicRecovered`'s is: a task spawned through the app's runtime, `Dep<dyn Runtime>` or `AppHandle::runtime()`, has every secret the graph registered replaced, its drop-time `warn` too, since the app binds the runtime as a wrapper of its own carrying the graph's redactor; one spawned on a runtime outside any app has the userinfo strip alone. The core logs, through `tracing`, at `warn` a panic raised while a task's future is dropped, which the wrapper `TaskHandle::launch` builds catches so that no runtime sees it, the task's end staying `Aborted` or what was recorded, at `warn` a panic raised while an execution's cache drops an execution-scoped instance at the end of the execution (§3.8), the first of the execution's, redacted with the graph's registry, and at `debug` a readiness backoff the runtime refuses, which ends that check (§9.3). The `warn` for a `serve` that fails once the shutdown has begun (§9.5) is the fourth site; nothing else in the core logs. `TaskHandle`, `TaskEnd`, the `RuntimeTask` SPI a runtime crate implements, `spawn_with` and `fw-transport`'s `TaskSet` are in transports §8.

When a `Timer` is configured, the core binds it as a global export under `dyn Timer`, a value in its own global module, so a service writes `Dep<dyn Timer>` and sleeps without a runtime dependency; a `Runtime` is bound under `dyn Runtime` beside it, and `dyn Timer` is then the same object, upcast, that object being the core's wrapper around `r` rather than `r`, which spawns and times on `r` and carries the graph's redactor, so nothing reaches `r`'s own type through either injection point or through `AppHandle::runtime()`, a downcast included, while `.runtime(r)` is the later of the two set; after `.runtime(r).timer(t)`, `dyn Timer` is `t` and `dyn Runtime` is `r`'s spawning with `t`'s clock, two objects that agree on the clock (§9.4). On an app with neither, either injection point is an ordinary missing dependency, whose report names the builder method, `.timer(..)` or `.runtime(..)`. The binding is app configuration, not a binding to replace: `override_value::<dyn Timer>` or `::<dyn Runtime>` in a test is a wiring error with the hint "set it with `TestApp::timer(..)`" or "set it with `TestApp::runtime(..)`" (§11), since an override of the runtime would split the one object, and a module binding `dyn Timer` itself is two sources for one key against the core's export (§10.1 step 3).

`Bound` is how a hook, a constructor or a factory states how long it may take. `Default` takes the app's default for that kind of bound: `hook_timeout` (10 s) for a hook, `construct_timeout` (30 s) for a construction and for one attempt of a readiness check (§9.3). `After(d)` is an explicit bound. `Unbounded` is no bound, as a variant rather than `Some(Duration::MAX)`: `Instant + Duration::MAX` panics, and a deadline computed from it would turn "unbounded" into a panic in the shutdown path.

Every bounded item, a hook, a construction or a readiness check, is in one of those three states. A trait const writes the variant. A binding handle leaves the item at `Default` by writing nothing, makes it explicit with `.timeout(..)`, or `.attempt_timeout(..)` as well on a readiness check, and writes `Unbounded` with `.unbounded()` (§9.1, §9.3). The states are exclusive on one item, and the handle's type enforces it: an item whose bound is written has no `.timeout(..)` or `.unbounded()`, so a second bound does not compile (§9.1, §12).

One rule for a bound and the `Timer`:
- The app defaults apply only when a `Timer` is configured. Without one, an item left at `Default` runs unbounded.
- An explicit bound, `After(..)`, `.timeout(..)` or `.attempt_timeout(..)`, needs a `Timer` and is a wiring error without one (§10.1 step 6). So is a builder knob set on an app with none, and so is a readiness `.backoff(..)` (§9.3): a written wait that does not wait is refused like any other.
- `Unbounded` needs no `Timer` and raises no error. The outer `shutdown_timeout` still contains it (§9.5): unbounded per hook is not unbounded overall.
- The drain is the one asymmetry. Without a `Timer` it is zero-length, and live executions are abandoned at once (§9.5). Hooks run unbounded because they are the application's own code; executions wait on work the application does not control, and a drain with no bound would let one execution that never ends hold shutdown open.
- A standalone deadline (§6.3) on an app with no `Timer` is stored, reported by `deadline()`, and never fires. The rule that separates it from `.backoff` is where the fault is visible: a written wait that cannot wait is refused when `wire()` can see it. A deadline is a runtime value passed to `execute`, whose only error is `Closed`, and refusing it would widen that error for one rare case on an app `listen()` already confines to standalone use (§9.5).

Expiry drops the future. A hook or constructor dropped mid-await stops at that await with its own state as it was, and anything it spawned keeps running, because dropping a future stops no work spawned elsewhere. An execution abandoned at the drain's end is cancelled and left running instead (§3.8): transports and subtasks still hold it.

---

## 4. Bindings [1–10]

A user's service, in the struct style:

```rust
#[injectable]                                  // Auto → singleton for providers [1]
pub struct UserService {
    repo: Dep<dyn UserRepo>,                   // trait object [4]
    db: Dep<PgPool, Primary>,                  // qualified [5]
    plugins: Many<dyn Plugin>,                 // collection [7]
    audit: Option<Dep<AuditLog>>,              // optional [13]
    #[construct(default)]
    retries: u32,                              // owned state [14]
}
```

The same service in the constructor style, which is the promoted one:

```rust
pub struct UserService { repo: Dep<dyn UserRepo>, db: Dep<PgPool, Primary>, retries: u32 }

#[injectable]
impl UserService {
    // picked automatically because it's named `new`; #[construct] marks any other name
    pub async fn new(
        repo: Dep<dyn UserRepo>,
        db: Dep<PgPool, Primary>,
        cfg: Dep<AppConfig>,
    ) -> Result<Self, InitError> {                         // async + fallible [15]
        Ok(Self { repo, db, retries: cfg.retries })        // owned state [14]
    }
}
```

A controller is declared with `#[controller]`, in either style, and its handlers in a `#[handlers]` impl on every transport (§7). `#[controller]` writes what `#[injectable]` writes and implements `Controller`, a mark with no methods, for the type where the type implements `Handlers`, whose `mount` `#[handlers]` writes. `#[module]` takes a type implementing `Handlers` under `controllers` and refuses a `Controller` under `providers`, and `#[module]`'s `controllers` and `#[handlers]` both refuse an `#[injectable]` type, pointing at `#[controller]` (§12). It takes no route prefix; a module mounts a controller under one with `m.controller::<T>().at(prefix)` (transports §2.5). On an impl, `#[controller]` and `#[handlers]` stack in either order, so `new` and the handlers can share one impl:

```rust
pub struct UsersController { users: Dep<UserService> }

#[controller]                                  // Auto → inferred for controllers [22]
#[handlers]
impl UsersController {
    fn new(users: Dep<UserService>) -> Self {
        Self { users }
    }

    #[fw_http::get("/users/{id}")]
    async fn get(&self, Path(id): Path<u64>) -> Result<Json<User>, UserError> {
        Ok(Json(self.users.find(id).await?))
    }
}
```

`#[injectable]` and `#[controller]` read and remove a field's `#[construct(default)]` and the `#[construct]` mark on a constructor, so rustc never resolves either path, and a field could not carry an attribute macro for it to expand. The field marker is one spelling under both attributes, and it names what the container does with the field: it constructs it from `Default` instead of injecting it (item 69). A field carrying `#[injectable(..)]` or `#[controller(..)]` is a compile error at it naming `#[construct(default)]`. Beside the item, the macro writes a hidden underscore import of each, `const _: () = { #[allow(unused_imports)] use ::fw::construct as _; };`, through which an editor answers hover and go-to-definition on the attribute as written; `#[handlers]` does the same for the enhancer and `#[meta]` attributes (transports DESIGN §2.1, which states the import's form).

The module ties these together:

```rust
pub struct Primary;
pub struct Replica;

#[module(
    imports   = [ConfigModule, DbModule::for_root(env_url("DB_PRIMARY")).keyed::<Primary>()],
    providers = [
        UserService,
        PgUserRepo as dyn UserRepo,                 // bind under a trait [4]
        AppConfig::from_env()?,                     // value [2]
        into dyn Plugin: [MetricsPlugin, TracingPlugin],  // collection contributions [7]
    ],
    exports   = [UserService],
)]
pub struct UsersModule;
```

And here is the same thing written against the value API. This is exactly what the macro expands to:

```rust
impl Module for UsersModule {
    fn identity(&self) -> ModuleIdentity { ModuleIdentity::of_type::<Self>() }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.import(ConfigModule);
        m.provide::<UserService>();
        m.provide::<PgUserRepo>().also_as::<dyn UserRepo>(|a| a);   // coercion in the closure
        m.try_value(AppConfig::from_env());                          // `expr?` in the macro
        m.contribute::<dyn Plugin>().provide::<MetricsPlugin>(|a| a);
        m.contribute::<dyn Plugin>().provide::<TracingPlugin>(|a| a);
        m.export::<UserService>();
    }
}
```

`expr?` in a `providers` list lowers to `m.try_value(expr)`: `register` never returns early (§3.6), and an `Err` is reported by `wire()` beside the other wiring errors. `expr` without `?` lowers to `m.value(expr)` and binds whatever type `expr` has, a `Result` included.

A `providers` list also takes the closure forms an `into` list takes: `with = closure`, a factory whose parameters are injection points and whose output type is the key, lowers to `m.with(closure)`, or `m.try_with(..)` for a closure answering a `Result`, the `Auto` pair `Contribute` has; `with(singleton | execution | transient) = closure` lowers to `m.singleton(..)`, `m.execution(..)` or `m.transient(..)`, or the `try_` form. A bare closure in a `providers` list is shorthand for `with = closure`, and so `Auto`: the same closure means the same thing in every position (§3.3). For a provider `Auto` behaves as a singleton, so what the bare form builds is unchanged; what differs is the record, which the needs-execution pass reads to name `with(execution)` in its hint (§6.2) and the hooks rule reads as a singleton's (§9.1).

A closure's output can also be provided under a trait key. The key comes first, as in an `into` list: `dyn Cache: with = |cfg: Dep<RedisConfig>| RedisCache::new(cfg)`, or `dyn Cache: with(scope) = ..`, lowers to `m.with(closure).also_as::<dyn Cache>(|a| a)` (the scoped method for `with(scope)`). As with `X as dyn T`, the concrete type stays bound in the module too, and exports decide what leaves it. The key comes first because a closure's body extends as far as it can: in `with = |cfg| RedisCache::new(cfg) as dyn Cache`, the `as` would parse as a cast inside the body. `K: [..]` contributes to a collection; `K: with = ..` provides a single binding. It is the spelling for a third-party type built by a closure, which `X as dyn T` cannot take because `X` must be a `Construct` type.

A qualified binding is written with the `@` that reports print for a key (§10.1): `PgUserRepo @ Primary` for a type and `Db::connect(&url)? @ Primary` or `Db("replica") @ Replica` for a value, lowered to `m.provide::<PgUserRepo>().qualified::<Primary>()`, `m.try_value(..).qualified::<Primary>()` and `m.value(..).qualified::<Replica>()`. The qualifier follows the entry, after `as dyn K` when both are written, and qualifies every key the entry binds: `PgUserRepo as dyn UserRepo @ Primary` binds `PgUserRepo @ Primary` and `dyn UserRepo @ Primary`, and leaves `PgUserRepo @ ()` unbound. An `exports` list takes the same suffix, `exports = [Db @ Primary, reexport Db @ Replica]`, lowered to `export_qualified` and `reexport_qualified`; `exports = [Db]` names `Db @ ()` and is `ExportNotBound` beside a binding that is only `Db @ Primary`. A qualifier is one type and written once: a second `@` is a span error, and a qualifier naming no type is rustc's own error on it. Two entries binding one type under one qualifier are a `DuplicateBinding` naming `Db @ Primary` (§10.1).

The slot is on the entries that bind a single instance from a type or a value. A closure binding (`with = ..`, a bare closure, `K: with = ..`) and an `into` item take none: `Db::connect(url)? @ Primary` is the value form, and a closure that must be qualified, like a readiness check, a closure hook (§9.1) or the `enhancer` mark (§7), is written in a hand-written `Module` against `ModuleDef`, where `.qualified::<Q>()`, `.ready(..)` and the hook methods are on the binding handle. The macro refuses an `@` after a closure or an `into` item with a message naming that call.

An `into K: [..]` list takes the entry forms `#[guards]` takes (§7), whatever `K` is: a type, lowered to `.provide::<T>(|a| a)`, its own binding with its trait hooks, unless the same `providers` list binds `T` unqualified, as `T` or `T as dyn X`, where it lowers to the doc-hidden `.reuse::<T>(|a| a)`, a factory contribution reading `Dep<T>` in `T`'s declared scope, so `providers = [Alpha, into dyn Plugin: [Alpha]]` builds one `Alpha` and runs its hooks once (§14, item 34); `value = expr`, lowered to `.value(Arc::new(expr))`, the `Arc::new` written by the macro; `value = expr?`, lowered to `.try_value(expr.map(Arc::new))`, which records an `Err` for `wire()` as a `providers` list's `expr?` does; and `with = closure`, a factory whose parameters are injection points, lowered to `.with(closure, |a| a)`, or `.try_with(..)` for a closure answering a `Result`, through the same autoref call site a `providers` factory takes (below). The closure's body is wrapped as `#[guards]` wraps it: a synchronous body in `async move`, an `async` closure or an `async` block kept, and both spellings compile. A `value` at module level is built once, because a module registers once; on a controller impl it is built once for the whole impl (§7). A `with` entry is `Auto` (§3.3). `with(singleton | execution | transient) = closure` writes its scope and lowers to `.singleton(..)`, `.execution(..)` or `.transient(..)`, or the `try_` form for a closure answering a `Result`; a word in the parentheses other than those three is a span error on it. Every `into` list lowers to `contribute`, and whether `K` is a role key is decided at freeze (§7). A `K` that implements `Meta` is module metadata, which a transport reads through `m.meta::<K>()` and never as a collection, so `into PreDispatch: [..]` is a compile error naming `K` and pointing at `ModuleDef`: a bound evaluated before the list's statements, because a proc macro sees no types. A braced providers entry, `{ LIMITS }`, lowers to `m.value(LIMITS)` with the braces taken off, since they only tell a constant from a type.

A malformed `into` item is a span error on the item, "an `into` entry is a type, `value = expr` or `with = |..| ..`", followed by "; an expression is contributed with `value = ..`" when the item parses as an expression: a call such as `MetricsPlugin::new()`, a transport-scoped form such as `http = AuthGuard`, or a key before `=` other than `value`, `with` and `with(scope)`. A `with` whose right side is not a closure reads "`with` takes a closure whose parameters are injection points, as in `with = |cfg: Dep<MetricsConfig>| MetricsPlugin::new(cfg)`".

Async factories and third-party types [3][8]:

```rust
// Parameters are injection points. The output type is the key. A `try_` factory's output is a
// Result whose Ok type is the key; its Err reports as `FailureReason::Errored`, on
// `ConnectError::Construct` for a singleton and on `LookupError::Construct` for a
// binding built inside a call (§3.4).
m.try_singleton(|cfg: Dep<DbConfig>| async move {
    PgPool::connect(cfg.url.expose()).await
});

m.try_execution(|pool: Dep<PgPool>| async move { UnitOfWork::begin(&pool).await });
m.transient(|| async { RequestId::new() });
```

Each scope has a plain and a fallible method: `singleton`/`try_singleton`, `execution`/`try_execution`, `transient`/`try_transient`. One method cannot serve both outputs, since blanket impls over `T` and `Result<T, E>` overlap, and a marker parameter only moves the failure to the call as a type-annotation error. The `#[module]` macro keeps one spelling: it writes the concrete call site, where autoref ranking over the closure's own type picks the fallible arm for a `Result` future and the plain arm otherwise.

A `Construct` type whose instance needs a custom build goes through `m.provide_with::<T>(factory)` or `m.try_provide_with::<T>(factory)`. The binding is keyed by `T`, uses `T::Scope`, builds through the factory, and runs `T::hooks`, which a plain factory never does (§9.1).

Aliases [6], where both keys reach the same object:

```rust
m.alias::<PgPool, ReadOnly>().of::<Replica>();             // `PgPool @ ReadOnly` reads the `Replica` binding
m.provide::<RedisCache>().also_as::<dyn Cache>(|a| a);     // second key under a trait
```

An alias keeps the type and changes the qualifier; the new key is in the type arguments and the existing one in `of`. A second key under another type is a coercion, which is `also_as`.

Trait coercion is written as `|a| a`. Stable Rust can't unsize generically, but a closure whose expected type is `fn(Arc<RedisCache>) -> Arc<dyn Cache>` coerces at its return. The macro's `X as dyn T` generates exactly this.

Duplicates [10]: a second single binding for a key in the same module, or a mix of `provide` and `contribute` under one key, is a wiring error naming both source locations.

A single binding under a role key the graph records is a wiring error on its own, with no contribution beside it: the pipeline reads enhancers through the collection alone (§7), so `AuthGuard as AnyGuard<Http>` in a `providers` list, `also_as::<AnyGuard<Http>>(|a| a)` on a binding handle, an alias of a role key, or a `provide`/`value` under one is bound and guards nothing. The report names the key and the binding, with the hint "contribute it". In `#[module]`, `X as <role key>` is refused earlier, at compile time, on the role key: "a role key takes contributions, and `as` binds a single instance; a global enhancer is written `into AnyGuard<Http>: [AuthGuard]`". That diagnostic reads the key as written: an alias such as `X as HttpGuards` passes the macro and meets the wiring error; a user's own type named `AnyGuard`, `AnyInterceptor` or `AnyErrorHandler`, or a `dyn` of a trait named `ErasedGuard`, `ErasedInterceptor` or `ErasedErrorHandler`, is refused after `as` although it is not a role key, and renaming the import binds it.

---

## 5. Injection points [11–17]

Every injection point obeys one rule: **an injection point is its type**. The container reads `T @ Q` for `Dep<T, Q>`, whichever identifier or alias the user wrote [16]. Fields, constructor parameters, factory parameters, enhancer closures, hook closures and readiness closures all accept any `FromContainer` type [11].

Reading per-execution data in an execution-scoped service [12][23]:

```rust
#[injectable(execution)]
pub struct AuditContext {
    user: Ext<CurrentUser>,             // written by a guard earlier in this execution
    head: Dep<fw_http::RequestHead>,    // execution input seeded by the transport
    exec: ExecutionRef,                 // cancellation + deadline
}
```

Shared instances [17]: `Dep<T>` is an `Arc<T>`, and every holder of a binding holds the same `Arc`, whether it's aliased, exported or re-exported. Mutation goes through interior mutability, and whatever an init hook does is seen by every holder. Services are never cloned. `Dep` clones the pointer.

Compile-time checks on injection points:
- A field or parameter whose type is not `FromContainer` produces that trait's diagnostic, which tells the user to write `Dep<T>` or set the value in the constructor.
- `Dep<T>` requires `T: Send + Sync`. The error is the auto trait's own ("`(dyn Repo + 'static)` cannot be sent between threads safely"), with no hint: rustc reports the unsatisfied `Send` from the bound's where-clause and drops any wrapper trait's `on_unimplemented` note. The fix, `Send + Sync` as supertraits of `Repo`, is documented rather than diagnosed. The other spelling mistake, a binding under `dyn Repo + Send + Sync` read as `dyn Repo` or the reverse, compiles on both sides and is caught at `wire()` (§10.1).
- `Ext<T>` or `ExecutionRef` in a type explicitly declared `singleton` fails the `AllowedIn` bound. The macro emits one `quote_spanned!` assertion per injection point so the error points at that field or parameter.
- A constructor whose future isn't `Send` fails at the generated `Construct` impl, pointing at the function.

A field or constructor parameter behind `#[cfg(..)]` is ordinary Rust, for `#[injectable]` and `#[controller]` in both forms. An attribute macro receives the `cfg` unevaluated, so the macro copies the field's or parameter's `cfg`s, and those a `cfg_attr` holds, onto its declaration in `dependencies`, its read in `construct` and the place the value goes, the call argument or the field initializer, a field marked `default` included: with the `cfg` off nothing names it, no dependency is declared and `wire()` reports none (§14, item 63). A tuple struct whose gated field would renumber the fields after it is built by position, `Self(..)`, each value under its field's `cfg`s; a wiring report names such a field by its index as written. A configured `#[module]`'s field behind a `#[cfg]` has its secret probe under the same `cfg`s; on a tuple module only the last field may carry one, and a gated field with a field after it is a span error naming the renumbering.

---

## 6. Scopes and executions [18–23]

### 6.1 Scope declarations

```rust
#[injectable]               // Auto
#[injectable(singleton)]    // explicit singleton
#[injectable(execution)]    // once per execution, shared inside it
#[injectable(transient)]    // fresh at every injection point
#[injectable(timeout = Duration::from_secs(5))]   // CONSTRUCT_TIMEOUT (§3.4); combines with a scope
```

Factories choose their scope with the method they're registered through: `m.singleton(..)`, `m.execution(..)`, `m.transient(..)`. Types that implement `Construct` always use `T::Scope`. Registration can't override it, which is what keeps the compile-time hook check sound.

### 6.2 Scope resolution at wiring time

After the cycle check, the graph computes whether each binding **needs an execution**. A binding needs one if any of its injection points is `Ext`, `ExecutionRef`, an execution input, or a dependency that itself needs an execution. Transient and auto bindings pass this need upward. The rules are then:

| Binding | Needs an execution | Result |
|---|---|---|
| Explicit singleton: `#[injectable(singleton)]`, `with(singleton) = ..`, `.singleton(..)` | yes | **refused** [21], with the full path |
| Auto provider, a type or a `with` closure under a provider key | yes | **refused**; the hint names the explicit form, `#[injectable(execution)]` for a type and `with(execution)` or `.execution(..)` for a closure |
| Auto controller or enhancer, a type or a `with` closure | yes | becomes per-execution, built per call [22][26] |
| Auto type with hooks that became per-execution | — | **refused** (the startup half of [41]); a closure hook or `.ready(..)` on its handle counts as a hook (§9.1) |
| Transient | yes | allowed, but every consumer must be able to run in an execution |

A hook, readiness, module-hook or metadata closure runs where no execution exists, so one reading `Ext<T>`, `ExecutionRef`, an input or a per-execution key is refused at `wire()`: nothing can ever satisfy it there, and `connect` is later and costlier. An enhancer closure runs inside an execution and is exempt.

### 6.3 Opening executions

Transports open one execution per HTTP request, WebSocket message, RPC call or gRPC call. A WebSocket connection is not an execution: per-connection state lives in `fw_ws::Session<T>`, a `FromContainer` type read from the execution input the WebSocket transport seeds [19].

Code opens standalone executions itself:

```rust
let report = app
    .execute(ExecOptions::new().deadline(Instant::now() + Duration::from_secs(30)), async |exec| {
        let job = exec.get::<ReportJob>().await?;
        job.run().await
    })
    .await??;                                   // `Closed` from Draining on, then the closure's own `Result`
```

```rust
impl App<Connected> {   // also on AppHandle and ModuleRef
    pub async fn execute<F, R>(&self, opts: ExecOptions, f: F) -> Result<R, Closed>
    where
        F: AsyncFnOnce(&Execution) -> R;
}
```

`execute` is refused from Draining on with `Closed`, a small public struct (§9.5). The closure borrows the execution, which stays owned by `execute`: `execute` drops it when the future completes, which ends the execution and drops its execution-scoped instances; a subtask that outlives the call takes `exec.handle()`, a clone that reads what the execution recorded and whose `get` then answers `LookupError::ExecutionEnded` (§3.8). A stream built from the execution, from a `Dep` it resolved or from `exec.handle()`, is consumed inside the closure: it ends when `execute` returns, and a read the stream makes afterwards answers `ExecutionEnded`. Completion of `execute` fires no cancellation; a standalone execution is cancelled at its deadline or at the end of the drain, and a subtask that should end with the job is awaited by the job. A plain closure returning an `async move` block cannot borrow its argument into the future, which is why the bound is `AsyncFnOnce` and the closure is written `async |exec|`. `execute` is an inherent `async fn` on every type that carries it, never a trait method: its future is `Send` through auto-trait leakage whenever the caller's closure future is, and a trait would have to write that `Send` bound, which stable Rust cannot state for an `AsyncFnOnce` future.

`App::execute` and `AppHandle::execute` resolve with the root module's visibility; `ModuleRef::execute`, reached through `app.module::<M>()?`, resolves with `M`'s (§8.2).

A standalone deadline is enforced on the app's `Timer`: `execute` polls `Timer::sleep` to the deadline beside the closure, cancellation fires when the sleep resolves and the closure runs on to its end, and a deadline already past fires before the closure first runs. On an app with no `Timer` the deadline is stored, `deadline()` reports it, and cancellation never fires from it; §3.9 states why that is accepted where a written `.backoff` is refused. `Execution::open` stores a deadline and enforces nothing: the transport that opened the execution does.

One kind of execution opens while `execute` and `Execution::open` are refused. A transport ending a connection during the drain still has that connection's cleanup to run, a gateway's disconnect handler for example, and the cleanup is itself an execution. `Execution::open_terminal(&DrainToken, ..)` opens one: allowed in Draining, counted in the drain and bounded by `drain_timeout` like any other execution. `DrainToken` has a private field and one source, the core, which hands it to each transport through `Server::drain` as Draining begins (§9.5). It is `Clone`, an `Arc` inside, because a transport opens terminal executions from connection tasks it spawned long before the drain and each needs its own copy; cloning creates no second source. The token proves who opens the execution and the phase check proves when. Anyone implementing `Server` holds one, a user-written transport included, since the rule is "only transports" and not "only these crates"; a token or a clone used after the drain has ended gets `Closed`.

### 6.4 Execution inputs

A transport declares the context types it seeds, together with the transport that seeds them, so the wiring pass knows both:

```rust
// inside fw-http's own global module
m.input::<RequestHead>().seeded_by::<Http>();
m.input::<ClientAddr>().seeded_by::<Http>();
```

At each call, the transport seeds them with `exec.seed(head)`, bounded `T: Send + Sync + 'static` (§3.8). Inputs are app-wide and belong to transports: a keyed module declaring one is a wiring error.

Wiring walks each handler's reachable execution-scoped bindings and checks every non-optional input against that handler's transport. A per-execution service reading `Dep<RequestHead>` on a path from an RPC controller is a wiring error naming the handler, the service and the input, printing the dependency path from the handler to the service that reads the input, and naming both transports: the handler's and the one that seeds the input (§10.1). A service shared across transports reads the input as `Option<Dep<RequestHead>>`, which is `None` where the transport did not seed it. An input several transports seed is declared by each of them through `Transport::inputs`, each naming the others with `also_seeded_by` (transports §2.10): the freeze merges two declarations of one key whose seeder sets are equal into one declaration carrying both seeders, and the check passes a handler of any. A module's `seeded_by` names one seeder. A second declaration of the key by another transport with a different seeder set, or a transport's declaration beside a module's with another seeder or beside a binding under the key, is the wiring error `InputConflict` (§10.1 step 2).

Standalone executions are the one runtime case, since nothing static says what they seed: one that doesn't seed an input which an injection point reads gets `LookupError::NotFound { kind: Input }` at runtime, and the error names the key.

The same walk answers a transport's `prepare`: `Mounted::handlers_reading::<T>()` lists the mounted handlers whose reachable execution-scoped bindings read the input `T`, one `InputReader` per handler and read. An entry carries the handler (`handler()`), its transport as a `TypeName` (`transport()`) and the steps after the handler (`steps()`): each binding between the handler and the read, then the injection point, as in `Audit (execution)`, ``Dep<ClientAddr> (field `addr`)``. No entry carries the handler step rendered. A refusal writes `{controller}::{handler} ({transport})` from the handler and the `TypeName` against its own report's names (§10.1), and the transport prints one way throughout that report. A transport whose deployment cannot seed an input it declares, an embedded HTTP app whose host supplies no peer address for `ClientAddr` (transports §3.8), refuses those handlers at startup naming the path to the service that reads the input, rather than failing their first call.

---

## 7. Enhancers and roles [24–28]

A guard is any type that implements the role trait:

```rust
#[injectable]                                   // Auto: singleton, or per-execution if needed
pub struct AuthGuard { sessions: Dep<SessionStore> }

impl Guard<Http> for AuthGuard {
    async fn can_activate(&self, cx: &HttpCx) -> Result<bool, BoxError> {
        let Some(user) = self.sessions.lookup(cx.head()).await? else { return Ok(false) };
        cx.extensions().insert(CurrentUser(user));     // read later as Ext<CurrentUser>
        Ok(true)
    }
}
```

Declaring enhancers by type, by value or by closure [26]:

```rust
#[handlers]
#[guards(http = AuthGuard)]                              // by type, HTTP handlers only: AuthGuard is Guard<Http>
#[interceptors(TimingInterceptor)]                       // every handler: TimingInterceptor is Interceptor<Http> and <Rpc>
impl UsersController {
    #[fw_http::get("/users/{id}")]
    #[guards(value = RateLimit::per_second(100))]        // by value: built once, shared
    #[guards(with = |u: Ext<CurrentUser>| RoleGuard::require(u, Role::Admin))]  // closure, Auto: per execution, it reads Ext
    #[guards(with(execution) = || RequestTimer::start())]                        // closure, explicit: fresh state per call
    async fn get(&self, id: Path<u64>) -> Result<Json<User>, UserError> { /* ... */ }

    #[fw_rpc::message("users.get")]
    async fn get_rpc(&self, req: Payload<GetUser>) -> Result<User, UserError> { /* ... */ }
}
```

A controller-level enhancer applies to every handler on the impl, strictly. `#[interceptors(TimingInterceptor)]` expands to `spec.interceptor::<TimingInterceptor>()` once per handler, and each expansion requires the role for that handler's transport: `Interceptor<Http>` for `get`, `Interceptor<Rpc>` for `get_rpc`. A handler whose transport lacks the role is a compile error at the attribute naming the handler: written `#[guards(AuthGuard)]`, the impl above fails on `get_rpc` with a message like "`AuthGuard` is not a guard for `Rpc`, needed by `get_rpc`; implement `Guard<Rpc>`". There is no "apply where the role exists": a guard that guards three of four handlers is the bug [25] exists to prevent. A controller that serves several transports scopes an enhancer with the transport-scoped form, `#[guards(http = AuthGuard)]`, which applies it to that transport's handlers alone and checks only their role.

`value = expr` is built once, where it is written, and shared by every call of what it is written on. On a method that is one handler. On the impl it is every handler of the impl: `#[handlers]` builds each impl-level value once when the controller mounts, into an `Arc`, and every handler's mount takes a clone of it (transports DESIGN X2), so a rate limiter declared on the impl limits the controller as a whole and not each handler alone. A known limit sits beside it: a controller-level entry scoped to a transport key no handler carries, `#[guards(htpp = AuthGuard)]`, applies to nothing and is not yet reported. `#[handlers]` expands before the transport attributes and does not know its handlers' keys; the report waits for the transport protocol to carry each transport's key.

In a `#[handlers]` impl, a method carrying any attribute other than the language's own (`doc`, `allow`, `warn`, `deny`, `expect`, `cfg`, `cfg_attr`, `inline`, `must_use`, `deprecated`, `track_caller`) or an enhancer attribute is a handler. A helper annotated `#[tracing::instrument]` is read as one and fails to compile, as a handler whose transport attribute read nothing; helpers go in a separate `impl` block. `#[handlers]` reads and removes the enhancer attributes and `#[meta]`, and writes a hidden import of each beside the impl for an editor to resolve them by (transports DESIGN §2.1).

Global enhancers are collection contributions under a role key [28]. In `#[module]` they are an `into` list, `into AnyGuard<Http>: [AuthGuard, value = RateLimit::per_second(100)]`, which lowers to `contribute` like every `into` list (§4); the value API writes `contribute` or `enhancer`:

```rust
m.contribute::<AnyGuard<Http>>().provide::<AuthGuard>(|a| a);   // a global HTTP guard
m.enhancer::<AnyInterceptor<Rpc>>().value(Arc::new(Tracing::default()));
```

The key decides the role, at freeze. `Mount::handler` records the `TypeId`s of its transport's three role keys. At the end of the freeze, once every controller has mounted its handlers, the role set is those keys for every mounted handler, a lazy load's base included, plus the key of every contribution marked through `enhancer`. Every contribution whose key's `TypeId` is in the set is an enhancer, through `contribute` or `enhancer`, under an alias or a `macro_rules` wrapper alike: scope inference (§3.3) applies to it, and no type name is read. One marked contribution marks the whole key, so one `m.enhancer::<K>()` anywhere in the graph marks every `into` list's contributions to `K` with it. The same pass refuses two forms under a key in the set, each of which nothing reads: a qualified contribution, `contribute::<AnyGuard<Http>>().qualified::<Q>()`, registered under `AnyGuard<Http> @ Q` where the pipeline walks `entries::<AnyGuard<Http>>()` unqualified (§3.2), with the hint "contribute it unqualified"; and a single binding, through `also_as`, an alias, `provide` or `value`, which the collection read never reaches, with the hint "contribute it" (§4). A role key no mounted handler reads and no `enhancer` marks is outside the set and is not checked.

`enhancer::<R: Role>()` is the explicit mark, for a role key of a transport with no mounted handler, which the recorded set cannot see: a shared auth module wired into a worker binary, or a test app without controllers. Unmarked, that contribution would be a provider, and an `Auto` entry needing an execution would be refused as a singleton (§6.2). `#[module]` has no spelling for the mark; an `into` list lowers to `contribute`. A module contributing enhancers for a transport the app mounts no handler of implements `Module` by hand, or relies on one `m.enhancer::<K>()` contribution written anywhere in the graph, which marks the key for the list's contributions too. The `Role` bound (§3.7) refuses a type that is not a role key. `enhancer` returns the builder `contribute` returns, marked: `Contribute<'m, U, Q, Mark = Plain>`, `Plain` from `contribute` and `Enhancer` from `enhancer`, every method shared across the mark but `qualified`, which exists on `Plain` alone. `Plain` and `Enhancer` are empty enums, exported at `fw::handle` beside the binding handle's state markers: no value of either exists, and code generic over the mark names no sealed trait. A role collection is read unqualified (§3.2), so `.qualified::<Q>()` on an enhancer contribution is E0599 at the call; the same qualifier through `contribute` is the wiring error above.

A `with` closure takes its scope from the axis in §3.3: `Auto` unless `with(scope) = ..` writes one, and the same moved between a method, the impl and an `into` list. `#[guards(with = f)]` lowers to `spec.guard_with(f)` and `#[guards(with(execution) = f)]` to `spec.guard_with_in::<scope::PerExecution>(f)`, `#[interceptors]` and `#[error_handlers]` to their kin, and an `into` list's entry to `Contribute::with` or the scoped method (§4). An `Auto` closure is inferred as a type is, from what its parameters read (§6.2): `with = |u: Ext<CurrentUser>| ..` above is built per call because it reads `Ext`, and the `RequestTimer` guard reads nothing and writes `with(execution)` to be built per call (§3.3).

Stack order is global, then controller, then method [26]. Within the global level, contributions follow collection order (§3.2).

The dispatch pipeline for one call [27] runs like this. Steps 2 to 4 are the core's `dispatch(handler, exec, cx, call)`, which every transport calls rather than writing the walk itself:

1. The transport opens an `Execution`, seeds the inputs, and builds its `Cx` around a handle to it.
2. `dispatch` reads the role collection through `Resolver::entries::<AnyGuard<T>>()`, which hands it one unresolved entry per contribution, and walks them in stack order together with the controller's and the method's declarations. For each guard it obtains that guard (a shared value, the singleton, `resolve().await` for a per-execution build, or the closure call), then runs `can_activate`. On a refusal it stops. **Later guards are never built.** `Many<AnyGuard<T>>` would build every guard first, which is why the pipeline reads `entries` and not `Many`.
3. Only once every guard admits does it build the interceptors, then the controller (per call if inferred), then run the handler inside the interceptor chain. A transport whose `Transport::BUILDS_CONTROLLER` is `false` skips the controller here and leaves it to the handler, which builds it inside the chain if it reads it: `fw-ws`'s `WsConnect`, whose handler is a gateway's connection phase and reads the gateway only to call a hook it implements, so a gateway reading a message's execution data connects, and a failure to build it at connect passes through the connect interceptors.
4. Errors go through the error handlers, method level first, then controller, then global. A refusal is one of them: the pipeline turns `Ok(false)` into the core error `GuardRejected { guard: &'static str }` and routes it like any other, so a handler that downcasts the `BoxError` can reshape the response per route. Unclaimed, the transport renders its forbidden status: 403 on HTTP, `PERMISSION_DENIED` on gRPC, an error reply on RPC, an error frame on WebSocket. Each transport crate documents its mapping. A guard that wants another status, 401 rather than 403, returns its own `Err`. A panic is one of them too. Each stage runs under its own catch, a guard, an interceptor, the handler and each error handler, and a panic becomes `PanicRecovered { stage, message }` (§10.2) where it happened, `stage` a `DispatchStage` naming which of the four; a `with` closure's panic counts as the stage of the enhancer it builds. A handler's panic passes back through the interceptors: the innermost interceptor's `next.run()` answers `Err(PanicRecovered { stage: Handler, .. })`, each interceptor further out receives what the one inside it answered, and an interceptor reshapes or answers it as it does any handler error. State the panicking stage touched may be inconsistent, so an interceptor treats that `Err` as fatal for the call rather than retrying. A panicking error handler's panic is offered to the handlers after it. A panic while the container builds an enhancer or the controller, a per-execution guard's constructor for example, is no stage's and arrives as `LookupError::Construct { reason: Panicked(..) }`; `fw::is_panic(&BoxError) -> bool` answers `true` for either shape, and for one a constructor's or a `try_` factory's own `Err` wraps (§10.2); an error handler mapping every panic to one status is one line. Unclaimed, the transport renders its internal-error status.
5. The transport drops its `Cx` when the reply is written. A streaming reply holds a clone, and the execution ends with the last one (§3.8).

HTTP middleware is declared as scoped `PreDispatch` entries with `exclude` (transports §3.3).

WebSocket gateways are controllers whose handlers carry `fw_ws` attributes, declared with `#[controller]` and a `#[handlers]` impl carrying `#[fw_ws::gateway(..)]`. A gateway participates in the container like any other controller [24].

---

## 8. Modules [29–38]

### 8.1 Declaring modules

```rust
#[module(
    imports     = [DbModule::for_root(url)],
    providers   = [UserService],
    controllers = [UsersController],
    exports     = [UserService, reexport PgPool],   // re-export from an import [34]
)]
pub struct UsersModule;

#[module(global, providers = [AppConfig::load()?], exports = [AppConfig])]   // global [30]
pub struct ConfigModule;
```

`#[module]` writes the declarative part of a module: what it imports, which types and values it binds, under which trait or qualifier, what it contributes to a collection, which controllers it mounts and what it exports. The rest of the value API has no spelling in it, and a module that needs any of it implements `Module` by hand (§3.6):

- a readiness check, `.ready(..)` and its `.retries(..)`/`.backoff(..)`/`.attempt_timeout(..)` (§9.3);
- a closure hook on a binding or on the module, `.on_init(..)`, `.on_destroy(..)` (§9.1);
- the `enhancer` mark for a role key of a transport with no mounted handler (§7);
- a qualified closure binding, `m.singleton(..).qualified::<Q>()`, and a qualified collection, `m.contribute::<K>().qualified::<Q>()`;
- module metadata, `m.meta::<T>()`, such as an HTTP pre-dispatch stage (`ulo_http::PreDispatch`) with its scoped, excluding and supplying entries, which is a builder over a typed value and no collection an `into` list contributes to.

A hand-written `register` and an attribute-written one are the same kind of module: `imports = [HandWritten]` and `HandWritten`'s own `m.import(Attributed)` both work, and the usual split is an attribute-written root importing a hand-written module for the part that needs the handle.

### 8.2 Visibility

A module can see its own bindings, the exports of its direct imports, and the exports of global modules. That's all. An export is the only way a binding leaves its module [29]. A re-export is allowed only for a key the module can see unambiguously [34].

Modules may import one another, directly or through other modules. An import decides what a module sees, not what is built first, and nothing reads the import graph as acyclic (§14, decision 29): a binding read across a cycle is an edge of the dependency graph like any other, and a cycle of those is refused (§10.1 step 4). A module importing itself is refused as `ImportCycle`, since it sees its own bindings without the import. A module inside a longer cycle inherits its qualifier (`ModuleDef::qualifier`, §8.3) by the rule a module two importers share follows: the first declared import path that reaches it decides, deterministic in declaration order (decision 29).

A lookup that names no module uses the root module's visibility: `app.get`, and `exec.get` inside an execution the app opened, see the root's own bindings, its imports' exports and the globals. The root's table is free of ambiguity once wiring passes, so no lookup variant is needed for it. A service exported only within a subtree is unreachable that way by design; it is reached through `app.module::<M>()?`, and a job that belongs to a subtree runs through that module's `execute`. Inside a transport call, the execution's resolver carries the dispatching controller's module: handlers and their per-execution services see what their module sees. Those tables are checked only where an injection point reads, so a lookup through `ModuleRef::get`, through `exec.get` in an execution opened on a module, or through `by_key` can meet a key two of that module's imports export; it answers `LookupError::Ambiguous { key, sources }` (§10.2), and `Option<S>` propagates it rather than answering `None`.

Collections are the exception: `Many<T>` gathers contributions from every module in the application, because plugins and global enhancers are app-wide by nature [7].

### 8.3 Configured and keyed modules

```rust
pub struct DbModule { url: Secret<String> }   // Eq + Hash → identity by value

impl DbModule {
    pub fn for_root(url: impl Into<Secret<String>>) -> Self { Self { url: url.into() } }
}

// two databases from one integration [32]
imports = [
    DbModule::for_root(primary_url).keyed::<Primary>(),
    DbModule::for_root(replica_url).keyed::<Replica>(),
]
// consumers read Dep<PgPool, Primary> and Dep<PgPool, Replica>
```

`Keyed<Q, M>` is itself a module. Inside `M`, injection points stay unqualified (`Dep<PgPool>`). At the export boundary, unqualified exports are requalified as `T @ Q`, re-exports included, since they leave the boundary like any export. Contributions are not exports and keep their key: two keyed databases contribute two `dyn HealthIndicator` entries. Execution inputs are app-wide and belong to transports; a keyed module declaring one is a wiring error.

Its identity includes `Q`, so the two instances are distinct modules, and a bare `DbModule::for_root(url)` beside `DbModule::for_root(url).keyed::<Primary>()` is two modules with two pools and two readiness checks. One pool under two keys is an alias (§4), not a second import. A module is keyed once: `store.keyed::<A>().keyed::<B>()` would carry `B` in its identity and leave its exports as `T @ A`, so `wire()` refuses it as `NestedKeyed`, naming both qualifiers (§10.1 step 1). A module that is not keyed itself reads the qualifier of the nearest keyed module on the path from it upward through `ModuleDef::qualifier`, answered before its `register` runs. A module reached by several importers is one instance and reads the key of the first declared import path that reaches it, and so does a module inside an import cycle: the answer is fixed at registration, in declaration order, and no handle filled at freeze revises it (decision 29).

A builder knob outside the configuration value, such as a store's connect bound or `.lazy()`, is not part of the identity: the options decide which pool a module is, and the knobs how it starts. The module answers its knobs from `Module::settings`, each a name and its value as `Debug` writes it, none by default, and `Keyed<Q, M>` answers `M`'s. Two imports of an equal identity are one module and only the first registers, so the second's settings would go unused: where the two `settings` differ, `wire()` refuses the second as `DifferingSettings`, naming the module, both import sites (the importing module and the `import` call's location, `ModuleDef::import` being `#[track_caller]`) and each setting that differs with both values (§10.1 step 1). Equal settings are one module. A struct declared `#[module(identity = type)]` with fields is identified by its type and its fields are compared by nothing, so it is registered once and a second registration is `RegisteredTwice` (decision 73). A keyed pair over equal options is two identities and may differ. A module loaded with `load` is compared the same way, against the module the graph holds under its identity, and so is each of its imports (§8.6); `AppHandle::load` is `#[track_caller]`, so its side names the `load` call. A module importing itself is reported as the `ImportCycle` alone: that import's settings are not compared, so a self-import with other settings is one entry. An import closing a cycle through other modules is a later import of an identity like any other, and its settings are compared. Each `DifferingSetting` holds both values as written, and the entry's text prints them after the graph's redaction function, so a secret the graph registered is replaced there as in every other error; anything else is printed as written, and a credential belongs in the identity, which is not printed. A URL a setting reports, a Redis URL, an OTLP endpoint or a proxy, is its scheme, host, port and path, never its userinfo, query or fragment, one rule the integration crates share through `ulo::secrets::setting_url`, public for the authors of module settings: the path stays because it identifies what the URL reaches, a collector's `/v1/traces`, a Redis database number or a Unix socket's location, so two URLs differing there compare unequal, and two differing only in userinfo, query or fragment compare equal. What a query parameter selects is reported as a setting of its own, parsed out of the URL, so the URL rule stays one rule: `ulo-ws-redis` reports a Unix socket's `?db=` as `database`, the number the redis client selects, beside `url`, and the ACL user the connection authenticates as, from the userinfo or `?user=`, as `username`, while a password stays out of every setting. A URL that does not read as a scheme and an authority, a port that is not digits, is its scheme and `[unreadable]`, and so is one whose kept text could precede an `@` of the original: an `@` after the start of the host, in the path, the query or the fragment, may close a userinfo holding an unencoded `/`, `?` or `#`, which would make the host reported a fragment of the password (§14, item 62). A secret a refused second registration alone declares is never registered, since that registration's `register` never runs; a credential written as RFC 3986 requires, its `/`, `?` and `#` percent-encoded, reaches neither the values nor the text even so, because the setting never holds it. An unencoded `?` or `#` inside the userinfo leaves the part before it reading as a host, and the `@` after the cut leaves the URL unreadable by the same rule. A list a client splits at its commas, NATS servers or Kafka brokers, goes through `ulo::secrets::setting_url_list`: each element by the URL rule, and an element is `[unreadable]` when a later element of the same URL holds an `@` or an earlier one holds a `?` or `#`, a URL running from an element that opens with a scheme or `//` to the next such element, since a password or a query value holding a comma is split by the client into such an element and the next. A module's settings name each setting once. `ModuleSettings::with` keeps a name's first value and records a repeat with both calls' locations, `with` being `#[track_caller]`, and `wire()` refuses the module as `DuplicateSetting`, naming the setting, the module and both lines (§10.1 step 1), so a module appending its own settings to an adapter's or an exporter's is refused where the two collide, at a startup phase, rather than comparing the first value alone.

### 8.4 Runtime-built modules [33]

```rust
pub fn redis_module(cfg: RedisConfig) -> DynamicModule {
    DynamicModule::new::<RedisIntegration>(cfg.clone(), move |m| {
        m.singleton(move || { let cfg = cfg.clone(); async move { RedisPool::connect(&cfg).await } });
        m.export::<RedisPool>();
    })
}
```

The identity is the owner type plus the configuration value, so these modules are still distinct Rust-typed identities. What the closure captures beside the value is opaque to the graph; `DynamicModule::with_settings(settings)` names it as the module's settings (§8.3).

### 8.5 Module handles [37]

```rust
async fn from_anywhere(app: &App<Connected>) -> Result<(), LookupError> {
    let users = app.module::<UsersModule>()?;                    // AmbiguousModule if several configs
    let replica = app.module_keyed::<DbModule, Replica>()?;
    let audit = app.module_of(&DbModule::for_root(audit_url))?;  // one configuration among several
    let pool = replica.get::<PgPool>().await?;                   // limited to what that module sees
    Ok(())
}

#[injectable]
pub struct PluginHost { here: ModuleRef }                        // the enclosing module
```

`module_of(&config)` answers the module whose identity is `config`'s, its type, configuration and qualifier together, so a keyed instance is named as it was imported, `module_of(&DbModule::for_root(url).keyed::<Primary>())`. It reads the graph and loads nothing: a configuration the graph does not hold is `LookupError::NotFound` with `LookupKind::Module`, where `load(config)` would wire and connect it. The three lookups live on `App<Connected>` and on `AppHandle` alike.

A `ModuleRef` carries `get` and `execute`, both limited to what its module sees. During `connect`, `get` for a singleton the eager walk has not built yet is refused with `LookupError::NotReady { key }`: building it on demand would break the eager order, and `NotFound` would misdescribe a key that exists. Once `connect` returns, `NotReady` cannot occur.

A `ModuleRef` or an `ExecutionRef` read during a construction carries that construction and the ones whose reads led to it. A `get` through either that comes back to an execution-scoped or transient binding still being built further up those reads is refused with `LookupError::Reentrant { key, reader }`, `key` the binding being built and `reader` the construction that read it back: the execution-scoped cell would wait on its own build, and a transient would recurse until the stack ran out. `wire()` cannot see such a cycle, since neither handle declares a key. A handle kept past its construction, by a closure the construction returns, carries a construction that has ended and refuses nothing (§14, item 33). A task the construction spawns with the handle reads inside the construction until the construction ends, so its read of the binding under way is refused rather than waiting for the build. An execution opened through such a handle, by `ModuleRef::execute` or `Execution::open`, carries the handle's construction too, and every resolver and handle it gives starts from it: a construction that reads itself back inside one is refused there as through the handle itself, although the new execution's cache holds no build of it. A handle kept past its construction opens executions that carry nothing under way.

### 8.6 Lazy modules [38]

```rust
let handle: AppHandle = app.handle();                 // Clone + Send + Sync; taken before serve(self)
let reports: ModuleRef = handle.load(ReportsModule).await?;   // Result<ModuleRef, LoadError> (§10.2)
```

`AppHandle` is a `Clone + Send + Sync` view of the shared inner state, available from `Connected` on. It is a view, not an owner: keep the `App`, or the future serving it, alive for as long as the app serves, since the `App` value's drop is the end of the app however many handles are kept (§9.5 step 6), and an embedded host serving through a kept handle then answers each request 503. `get`, `module`, `module_keyed`, `module_of`, `execute`, `load`, `close`, `draining`, `is_draining`, `drain_timeout`, `timer` and `runtime` live on it, `timer()` and `runtime()` each answering `Option<&Arc<..>>`, the objects `Dep<dyn Timer>` and `Dep<dyn Runtime>` resolve to, for code holding a handle rather than a `Dep` (§3.9), `draining()` returning the same `Draining<'_>` an execution's does (§3.8) and resolving at the same moment, and `drain_timeout()` the window the drain runs under (§9.5), zero on an app without a `Timer`, which a transport hands to a host that stops on a clock of its own (transports §3.8); `serve(self)` consumes only the `Bound` typestate, so a handle taken before `serve` keeps working while the app serves. `close` on a handle is one of shutdown's three triggers and ends `serve` (§9.5). The graph sits behind a lock that only `load` writes.

A lazily loaded module is wired against the frozen graph, with all of its errors reported in one pass, and then connected through its own readiness checks and init hooks. Loading the same identity twice returns the existing handle, when the settings are equal; with other settings it fails with `LoadError::Wiring` holding `DifferingSettings`, its second site `ModuleSite::Load` naming the `load` call (§8.3). A `#[module(identity = type)]` module with fields loaded again is `RegisteredTwice`, whatever its fields hold (decision 73). The modules a load brings may import one another and the frozen graph's modules; no frozen module imports them, so a cycle stays inside one load.

`load` fails with `LoadError` (§10.2): `Wiring` for the module's wiring errors, `Connect { error, teardown }` for a construction, readiness check or init hook that fails, `Refused(LoadRefusal)` for what the frozen graph cannot take, and `Closed` by phase. `LoadRefusal` names five things, refused because the graph has already been handed out:
- **Controllers and middleware**, because routes are already bound.
- **Contributions to a collection the module does not introduce itself.** A lazy module cannot contribute to any key that a pre-existing binding reads as `Many<T>`, whatever that binding's scope; the rule is static and checked at `load` without asking what has already run.
- **Global exports.**
- **Execution inputs.** Inputs belong to transports (§6.4), and the per-handler input check ran at wiring time over the inputs the graph then had.

Shutdown includes lazily loaded modules, in reverse order of loading. `load` is refused with `LoadError::Closed` from Stopping on (§9.5): a module loaded then would miss the before-shutdown stage already running, and its shutdown would be incomplete.

---

## 9. Lifecycle [39–43]

### 9.1 Hooks

For types the container constructs, hooks are trait implementations:

```rust
#[injectable]
impl Cache {
    pub fn new(pool: Dep<PgPool>) -> Self { /* ... */ }
}

impl OnModuleInit for Cache {
    async fn on_module_init(&self) -> Result<(), BoxError> { self.warm().await }
}
impl OnApplicationShutdown for Cache {
    const TIMEOUT: Bound = Bound::After(Duration::from_secs(3));   // `Default` when left out (§3.5)
    async fn on_application_shutdown(&self, signal: &Signal) { self.flush().await }
}
```

`#[module]` has no spelling for a closure hook or a readiness check; a module that needs one is written against `ModuleDef` (§8.1).

For factory outputs (such as a third-party pool), hooks are closures registered on a **singleton** binding handle, the handle of an `Auto` provider included, since an `Auto` provider is a singleton (§3.3); for modules, on the `ModuleDef` (§13). The binding handle is typed, so the hook methods and `.ready(..)` don't exist on execution-scoped or transient handles [41], and an `Auto` binding that inference moves per-execution with any of them written is refused at `wire()`, the readiness check counted as a hook (§6.2):

```rust
m.try_singleton(|cfg: Dep<DbConfig>| async move { PgPool::connect(cfg.url.expose()).await })
    .timeout(Duration::from_secs(20))                                        // bounds construction
    .on_destroy(|pool: Dep<PgPool>| async move { pool.close().await })
    .timeout(Duration::from_secs(3));                                        // bounds the hook

m.on_init(|users: Dep<UserService>| async move { users.seed_admin().await })   // module hook [39]
    .timeout(Duration::from_secs(5));
```

The handle is one type with a state parameter naming the item written last and whether its bound is written, and the binding is the first item: `.timeout(..)` and `.unbounded()` write the bound of whatever the state names, construction directly after `singleton(..)`, the check after `.ready(..)`, the hook after `.on_destroy(..)`, and either call moves the item to a state that has neither method. A second `.timeout`, or `.unbounded()` beside a bound, is E0599 at the second call (§12). `.retries`, `.backoff` and `.attempt_timeout` exist on the readiness item alone, the first two in every one of its states and repeatable, the last write winning (§9.3). `also_as` and `qualified` describe the binding and return to it in the state it was left in: the handle carries the construction's state across the other items for that return, and the construction's bound is written once wherever the handle is on the binding. Misuse is E0599 at the method, naming the handle type the method exists on and the state bound it lacks. A closure's `.timeout` means what the trait const means (§3.9), `Default` when left off, and `.unbounded()` writes `Unbounded`.

A closure hook reads its parameters before it runs, and a read that fails means the hook never ran: the failure is recorded as `FailureReason::Errored` holding the `LookupError`, on `ConnectError::Hook` for an init or bootstrap hook and on `ShutdownFailure::Hook` for a destroy or shutdown hook (§10.2).

A factory's output is a plain value, so trait hooks run only for types the container constructs. This rule is documented, and it's the reason closure hooks exist. It holds even when the output type implements `Construct`: `m.singleton(|| async { Cache::custom() })` binds `Cache` by factory, and its `OnModuleInit` impl compiles and never runs, because the graph learns which types are `Construct` only through `provide`. A `Construct` type that needs a custom build is bound with `m.provide_with::<Cache>(|| async { Cache::custom() })`, which runs `Cache::hooks` like `provide` does (§4).

`Hooks<T>` has one method per hook trait, each with a `where` bound, so `h.on_module_init()` compiles only when `T: OnModuleInit`, and likewise `on_application_bootstrap`, `on_module_destroy`, `before_application_shutdown` and `on_application_shutdown`. A hand-written `Construct` impl knows which traits it implemented and calls those methods. `#[injectable]` fills `hooks` with autoref probes over the concrete `Self`; the probe types are macro internals, and `fw::hooks!(h)` is the same probing offered as a macro for a hand-written impl that wants all five checked. The registration reads each hook's bound fully qualified, and that is also how user code reads one on a type implementing several hook traits: `<T as OnModuleInit>::TIMEOUT`; `Self::TIMEOUT` is ambiguous there.

### 9.2 Order [40]

The connect phase walks the singleton graph in a stable topological sort: among the bindings whose dependencies are done, the smallest (module post-order index, declaration index) runs next, the module index being the module's position in collection order (§3.2). For each binding it constructs the instance and runs that binding's readiness check. It then runs all `OnModuleInit` hooks in the same order. A module's own hooks run after the hooks of its providers. Last come all `OnApplicationBootstrap` hooks. Two bindings with no edge between them are ordered by the tie-break, so the order is the same on every run. The order reads binding dependencies and collection order, which is total on an import graph with cycles (§3.2), so an import cycle changes no rule: a singleton reading another module's export across the cycle starts after it, whichever of the two the walk placed first.

Close runs the hooks in the exact reverse, with the drain between the first and the rest: `BeforeApplicationShutdown(signal)` while the app still serves, then stop accepting and drain, then `OnModuleDestroy`, the socket close and `OnApplicationShutdown(signal)` (§9.5).

The first failure stops the walk, and what started is torn down (§14, items 31 and 60). The `OnModuleDestroy` hooks of every binding and module that had initialized run in the exact reverse of their startup order. An owner with an init hook has initialized once that hook completes. A binding with none has once it is built and checked, and a module with none once every singleton it registers has initialized, wherever either sits relative to the failure (§14, item 64); either also waits for every singleton its destroy hooks read, a transient followed through what it builds and a value with no readiness check counted from `wire()`, so no destroy hook runs whose dependency was never built (§14, item 65). So an `OnModuleInit` failure tears down the owners before the one whose hook failed, every destroy-only binding after it, and every module after it with no init hook whose singletons all initialized; a bootstrap hook's or the `dyn TraceSource` read's failure tears down every owner; and a construction or readiness failure tears down the destroy-only bindings built and checked before it, the binding whose check failed not among them, and each module with no init hook whose singletons are all among those, each only when the singletons its destroy hooks read are among those too, since init hooks run only once every singleton is built and checked. Each hook runs under its own bound and the whole teardown under a `shutdown_timeout` cap started as it begins, as at close (§9.5): a hook that panics, times out or, a closure hook, fails to read a parameter is recorded, hooks the expired cap reaches are `Skipped`, and every hook is attempted. No `BeforeApplicationShutdown` or `OnApplicationShutdown` hook runs, the app never having served. `connect` then returns `StartupError::Connect { error, teardown }`, the walk's failure in `error` and what the teardown recorded in `teardown`, empty when it recorded nothing (§14, item 66), and drops the app, and the instances release as they drop. A failed `load` tears down what it initialized the same way, then restores the base graph and removes the singletons it built, so a retry wires again.

### 9.3 Readiness checks [42]

```rust
m.singleton(|cfg: Dep<DbConfig>| async move { PgPool::connect_lazy(cfg.url.expose()) })
    .ready(|pool: Dep<PgPool>| async move { pool.ping().await })
    .retries(5)
    .backoff(Duration::from_millis(500))
    .attempt_timeout(Duration::from_secs(2))    // one attempt
    .timeout(Duration::from_secs(10));          // the whole check, retries and backoff included
```

A readiness check runs right after its binding is constructed and before anything that depends on it. `.timeout` bounds the whole check, retries and backoff included, so it means what a hook's `.timeout` means; a backoff the runtime refuses ends the check at once, logged at `debug`, reporting the refusal as `runtime_shut_down`, `Permanent`, with the last attempt's error as its source, or the runtime's `Refused` when the last attempt timed out, so a check never reads as having run out of attempts it did not use, and reporting the whole-check bound's `TimedOut { limit: Item }` instead when that bound had passed on the clock by then (§3.9); an attempt bound or a whole-check bound the runtime refuses ends the check at once too, with no backoff, as `Errored` holding a `Permanent` `RuntimeShutDown` whose source is the runtime's `Refused`, under §3.9's ties; `.attempt_timeout` bounds one attempt and is what catches a single hung ping. Both are optional, and both need a `Timer` and are wiring errors without one (§3.9); so does `.backoff`, which waits between attempts and cannot without one, and the report names the `.backoff(..)` call, which records its own location, rather than the `.ready(..)` before it. A check that writes neither takes `construct_timeout` as its attempt bound when a `Timer` exists, under the rule constructors follow: external I/O is what a readiness check exists for, and a ping to a host that drops packets never returns, so an unbounded first attempt would never reach its retries. Without a `Timer` it runs unbounded. Writing `.timeout`, `.attempt_timeout` or `.unbounded()` opts out of that default. Each of the two is written once, in either order, and `.unbounded()` stands in for both: it exists only while neither is written, and after it neither exists (§9.1). `.unbounded()` runs the check with no bound on an attempt or on the whole, for a dependency the app waits on however long it takes; `.retries` and `.backoff` keep their meaning under it. Neither of those two is once-only: they exist in every state of the item, and a repeated `.retries` or `.backoff` is last write wins. A repeated bound would conflict with itself, which is what the typestate refuses; a repeated count is redundant, and refusing it would cost states. An attempt that times out with retries left is retried like one that returned `Err`. A check that fails reports as `ConnectError::Readiness` with the attempts made and a `reason` for how it ended (§10.2): `TimedOut { limit: Item }` when the whole-check bound fired, whatever the attempt underway was doing, or had passed when the runtime refused a backoff; `TimedOut { limit: Attempt }` or `TimedOut { limit: Default }` when the last attempt hit its own bound with no retries left; `Errored(..)` holding the last attempt's error when the retries ran out on errors, or a `Permanent` `RuntimeShutDown` when the runtime refused a backoff or a bound; `Panicked(..)` when an attempt panicked, which ends the check at once.

Every error the core did not create itself, from user code, integrations or transports, passes through one redaction function before it is stored in any core error type, and what is stored is a `Redacted` (§10.2): the `Errored` and `Panicked` payloads of a `FailureReason`, a panic message and a transport's `close` error among them, `Bind`'s `source`, and the `Err` a `try_value` recorded in `WiringErrors`. The scope is the error's origin, not a list of variants, so a variant added later is inside it with no list to extend. A task spawned through the app's runtime takes the same path: its panic is redacted with the registry the graph holds, which `wire` sets and every `load` refreshes, for the graph it publishes and for the base graph a failed connect puts back, read at the panic rather than at the spawn (§3.9). The function replaces every `Secret<_>` registered with the graph and, as a backstop, strips the userinfo from anything shaped like a URL. On the `try_value` path the registry has nothing to replace: the secret that would have been registered sits inside the config that failed to load. The strip is the only protection there, a backstop and not a guarantee. A `Redacted` writes the redacted text under `Display` and `Debug` alike, answers `None` from `source()`, and hands over the original through `downcast_ref` and `original` alone (§10.2). Registration is explicit per graph, never a process-wide list, which would leak between tests:
- `m.secret(&self.url)` in the value API, for a secret a module holds in its configuration and moves into a factory.
- `#[module]` registers the secrets a configured module's fields hold, deciding by each field's type rather than its spelling: the generated `register` calls an autoref probe on every field, which reaches a `Secret<T>` through an alias or a path and an `Option`, `Vec`, `BTreeMap` or `HashMap` of one, and falls to a no-op for any other type without bounding it, as the reply probe does (transports DESIGN §2.3). A held secret's `T` is required to be `Display` at the field. A `Secret<_>` inside another type, a struct of the application's holding one, is not reached: `m.secret(..)` in a module written with `ModuleDef`, or a configuration read, which registers each secret it deserialises, covers it.
- `m.value(..)` registers the value it binds when it is a `Secret<String>` or a `Secret<&'static str>`, the two a value of any type can be recognized as by downcast; any other `Secret<T>` needs `m.secret(&s)` beside it, or its text stays visible in reports.

A secret shorter than four characters stays registered and is matched against no message: a text that short is contained in most messages, and replacing it would mangle every report without hiding anything a reader could not guess. `ulo-config` applies the same floor to the messages it scrubs.

### 9.4 Phases [43]

```rust
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let app = App::builder(AppModule)
        .runtime(fw_runtime_tokio::Tokio::current())       // the clock and the executor; `listen()` needs one (§3.9, §9.5)
        .drain_timeout(Duration::from_secs(5))      // in-flight executions at shutdown; 10 s unset (§9.5)
        .shutdown_timeout(Duration::from_secs(25))  // the whole close sequence; unset by default (§9.5)
        .wire()?;                                   // graph + all wiring errors; no instances, no I/O

    let app = app.connect().await?;                 // outbound connections, readiness, hooks

    let app = app
        .bind(fw_http::Server::new("0.0.0.0:8080"))
        .bind(fw_grpc::Server::new("0.0.0.0:50051"))
        .listen()
        .await?;                                    // sockets

    let handle = app.handle();                      // AppHandle: get, module, execute, load, close, draining, drain_timeout, timer, runtime
    app.serve(fw_runtime_tokio::shutdown_signal()).await?;  // until the signal or `handle.close(..)`; returns the `Shutdown` report
    Ok(())
}
```

`hook_timeout` (10 s unset) and `construct_timeout` (30 s unset) sit beside those two knobs and are the `Default` every hook and construction starts with (§3.9); `construct_timeout` also bounds one attempt of a readiness check left at `Default` (§9.3). All four are timed by the `Timer` and apply only with one; set on a builder with none, each is a wiring error. `construct_timeout` and `hook_timeout` are refused at zero, since every item left at `Default` would time out before it starts, while `drain_timeout` and `shutdown_timeout` accept it, being windows where zero means none.

`.runtime(r)` sets the spawning and the clock, and `.timer(t)` the clock alone, enough for an app that binds no transport. Each call overrides only what it sets, applied in order: `.timer(t).runtime(r)` spawns and times on `r`, and `.runtime(r).timer(t)` is `r`'s spawning with `t`'s clock, each bound as `dyn Runtime` through the core's composite, which carries the graph's redactor for a task's panic too (§3.9), so the clock services read, the one the core's waits read and the one a transport sleeps on through the runtime are never two, and a real executor under a test's clock is those two calls (§3.9, transports §8).

A job, a CLI command or a test stops after `connect()` and uses `get` or `execute`, then calls `close(Signal::new("done"))`, which runs the shutdown sequence with no sockets to close (§9.5). Anything that needs the app while it serves holds an `AppHandle` taken before `serve` (§8.6).

### 9.5 Shutdown

Shutdown is one event per app. It has three triggers: the signal future passed to `serve` resolving, `close(signal)` on the app or on any `AppHandle`, and a transport's `Server::serve` ending while the app runs. The first to arrive starts the sequence, and its `Signal` is the one `BeforeApplicationShutdown` and `OnApplicationShutdown` receive. `handle.close(..)` ends `serve`: the serve future runs the sequence and returns its outcome.

**A transport whose `serve` ends starts the shutdown.** `serve` polls every server's `serve` future, and one that returns `Err`, panics, or returns `Ok` before any other trigger has arrived, while the phase is still `Running`, is a trigger like the other two: a process that keeps serving its remaining transports answers less than it advertises while every liveness signal reads healthy. Its signal names the transport, `transport `<name>` failed: <redacted error>`, `transport `<name>` failed: panicked: <redacted message>` or `transport `<name>` stopped serving`, and the sequence that follows is the same one: every server's drain and `close` run, the dead server's included, and every hook runs. The outcome is `Err`, its first failure `ShutdownFailure::Serve { transport, end }`, `end` being `ServeEnd::Errored` with the error redacted, `ServeEnd::Panicked` with the redacted panic message or `ServeEnd::Returned`, followed by whatever the steps record; `serve` returns it and every `close` caller receives it. The trigger and its failure are recorded under one lock, so a `serve` that ends as a `close` or the signal wins is part of that shutdown and records nothing, and a `serve` ending once the shutdown has begun is not a failure, a transport's accept loop returning at the drain being the normal end. A `serve` that returns `Err` or panics once another trigger has arrived, on the poll that trigger won or later, is not in the outcome either, and is logged at `warn` with the transport and its redacted error or panic message, so the failure leaves a trace. A panic in `serve` is caught on each poll with `catch_unwind` and redacted as `PanicRecovered`'s is; the panicked future is dropped and never polled again, and the other servers drain and close as for an `Err`. An embedding's host future ending before the shutdown is the embedding's `serve` returning `Err` (transports §3.8).

A `close` during a running shutdown starts nothing. It waits for the same shutdown and returns the same outcome. Its own signal is ignored, as is `serve`'s signal arriving during a handle-initiated close.

The sequence:

1. `BeforeApplicationShutdown(signal)` hooks, in reverse connect order, while the app still serves. Traffic is normal through this stage: a service that deregisters from discovery and waits for the last routed request to arrive does it here.
2. Stop accepting. `draining()` resolves and `is_draining()` turns true on every execution and every `AppHandle`. The core then hands each transport a `DrainToken` through `Server::drain` and each does the protocol's equivalent, a moment after `draining()` resolves rather than at the same instant; a request arriving in between is refused, since no execution opens from Draining on (step 3): HTTP/2 and gRPC send GOAWAY, HTTP/1 closes idle keep-alive connections, and WebSocket closes idle connections with 1001 at once while a busy connection stops reading, finishes the messages it has in flight and then closes with 1001.
3. Drain. The app waits for live executions to end on their own, up to `drain_timeout`. Nothing is cancelled in this window. New executions are refused with `Closed`, except the terminal ones a transport opens with its token for a connection's cleanup (§6.3), which count in the drain like any other.
4. Cancel and abandon the rest. At the timeout, cancellation fires on every execution still alive, transport-opened and standalone alike, and each is abandoned: it keeps its cache and its instances until its holder drops the `Execution` (§3.8), and the outcome counts it unless its answer is committed. A transport commits an answer with `commit()` on the execution immediately before the answer's final write (transports decision 181), so an execution whose final write had begun when the drain's end fired (the mark is read before the cancellation fires: the answer a cancelled call writes in response is the drain's doing and does not unabandon it), ending a moment later as its holder drops it, is cancelled and not counted; the write proceeds as the pipeline-first rule has it, and a write that then fails is a lost write, not an abandoned call. The drain also ends early when the `shutdown_timeout` cap expires, and the end of the drain is then that moment rather than `drain_timeout`; cancellation has no fourth source.
5. `OnModuleDestroy` hooks, in reverse connect order. From the first of them on, a fresh singleton lookup from an abandoned execution answers `LookupError::Closed`, and an instance it already holds stays a valid object whose destroy hook may have run.
6. Transports close their sockets. `close` is a promise to each server: it runs once on every shutdown path that reaches this step, the signal, a handle's `close`, a transport's `serve` ending and an expired cap alike, since it is where TLS close-notify, broker acknowledgements and deregistration happen, and a drop does none of them. The closes run concurrently, each bounded by what is left of `shutdown_timeout` as the step starts, or by `hook_timeout` when no cap is set, so a server waiting out a grace for its connections does not hold back the next one and the step lasts as long as its longest `close`. A `close` can wait on a TLS close-notify or a broker's acknowledgment, and one that exceeds its bound is dropped and recorded as `ShutdownFailure::Close` with `reason: TimedOut`, the limit `ShutdownCap` under the cap and `Default` under `hook_timeout` (§10.2), and one whose bound the `Timer` refuses is dropped at once and recorded `Errored` holding a `Permanent` `RuntimeShutDown` (§3.9); one that returns an error is recorded with `Errored`, and one that panics is caught and recorded with `Panicked`, as a hook's panic is. Each failure is recorded per transport, and the report lists them in bind order once every `close` has ended, whichever ended first, so it reads the same on every run. `close` is called inside the caught future, so a `Server::close` that panics before returning its future is caught too. Every `close` starts, and `Skipped` never occurs on `Close`. Once every `close` has run, the app lets go of its servers: a server drops with its last other holder, `serve` returning, or at once when nothing else holds it, and the app's shared state, its singleton store among it, drops with its last `AppHandle`. A server keeping the `AppHandle` its `prepare` is handed, as every shipped transport's does, holds the app while the app holds the server, and the release breaks that cycle (F425, transports 146). From then on `App<Bound>::addresses()` answers an empty list. Dropping the `App` value with no `close` claim held lets go of the servers the same way, and for an app that never reaches this step that drop is each server's shutdown: an app bound and dropped unserved, one whose `serve` future is dropped and one whose shutdown is abandoned each release their servers, every server dropping with its last other holder and its `close` not run (F426, transports 149). `AppHandle::close` is a `fn` returning its future, `Send` and borrowing nothing from the handle: the call claims the servers and the future's first poll triggers the shutdown. A drop while a `close` holds its claim leaves the servers to it, its future polled before the drop or first polled after it: the shutdown runs its hooks, this step closes every server and then releases them, and a `close` dropped before this step, polled or not, with the `App` value gone and no other claim held, releases them unclosed as it drops, the servers' drops running on the thread dropping it (transports 153). A `close` polled once and then kept unpolled holds the servers for as long as it is kept. An `AppHandle` kept past a release by a drop keeps the shared state, its singletons among it, and no server, and a `close` called on it then, this step not ended, runs none of the steps and answers `ShutdownError` carrying `ShutdownFailure::Dropped`, which states that the app was dropped before its shutdown closed its servers and that no shutdown hook runs after the drop. A release by a drop before this step ended, the `App` value's or the last claim's, moves the phase to `Closed`, under the lock the claims take, so a `close` answering `Dropped` is never followed by an earlier phase, and then, outside that lock, fires `draining()`, so a loop waiting on it through a kept handle wakes; from then on that handle's `execute` answers `Closed` and a lookup `LookupError::Closed`, as after a shutdown, and no hook runs. Once this step has ended, a `close` after the drop answers that shutdown's outcome. A `close` future holds a claim on the servers and the app's shared state, so it is awaited or spawned where it is made: one stored in state the app owns, a singleton among it, holds the app from inside the app, and no drop reaches either. The RPC links abort the loops holding their sockets and connections when they drop, and a broker server's side clears the calls it holds, so a server's drop closes its listener or its broker connections, with a call held at the drop or without (F428, F429, transports 47, 151, 152, 154).
7. `OnApplicationShutdown(signal)` hooks, in reverse connect order.

| Phase | New executions | Singleton lookups | `load` |
|---|---|---|---|
| Running | allowed | allowed | allowed |
| Stopping (step 1) | allowed | allowed | refused, `LoadError::Closed` |
| Draining (steps 2–4) | refused, `Closed`; terminal executions allowed | allowed | refused |
| Destroying onward (steps 5–7) | refused | `LookupError::Closed` | refused |

`is_draining()` is `false` throughout Stopping. Draining means new work is refused and the drain window has started; a stream told to end during Stopping would end before the discovery hook has deregistered, which is what the hook's position exists to prevent. Early notice is `BeforeApplicationShutdown`.

**Bounds.** Each hook runs under its own `TIMEOUT` (§3.9), `hook_timeout` where it writes none. The drain runs under `drain_timeout`, ten seconds unset. `shutdown_timeout` caps the whole sequence and is unset by default. Hooks run one after another: twenty hooks at ten seconds each take more than three minutes, while an orchestrator kills the process at its grace period, thirty seconds in Kubernetes, mid-hook and with nothing reported. The cap starts at the trigger, where the orchestrator's clock starts too, and the before-shutdown stage counts against it; a deployment sets it a few seconds under the grace period. When it expires, every remaining step that waits on user code is skipped: hooks not yet run are recorded as `Skipped`, a hook mid-run is dropped and recorded as `TimedOut` with `limit: ShutdownCap` and the cap's duration as `after` (§10.2), and a drain still open cancels and abandons at once. Stop accepting still runs, as it runs no user code. Each transport's `close` still starts, under what is left of the cap: nothing once it has expired, so a `close` that does not complete in its first poll is dropped and recorded as `ShutdownFailure::Close`. A hook declared `Unbounded` is still inside the cap. A cap the `Timer` refuses, its runtime shut down, acts as a spent one: the hook or `close` it bounds is cut at once, a `close` still getting its first poll, and is recorded `Errored` holding a `Permanent` `RuntimeShutDown` rather than `TimedOut` (§3.9).

**Abandoning a hook** drops its future (§3.9). The hook stops at its next await with its own state as it was, and anything it spawned keeps running. An execution abandoned at the drain's end is cancelled and left running instead: transports and subtasks still hold it. A hook that times out, has its bound refused by the `Timer`, is skipped by the cap, panics, (an init or bootstrap hook) returns an error or (a closure hook) fails to read a parameter is recorded with that reason (§10.2), and a failing step never stops the sequence. The outcome is `Result<Shutdown, ShutdownError>` (§10.2), received by `serve` and by every `close` caller. `ShutdownError` is `Clone` with its contents behind an `Arc`, because one outcome has several receivers.

**Disconnect handlers at shutdown are best effort.** A terminal execution follows the drain's rules: one opened a moment before `drain_timeout` is cancelled almost at once, and a connection still busy at the timeout has its cleanup refused, since its terminal execution would open after the drain has ended. No grace window follows the drain to save them. A handler opened at the end of such a window would have the same problem, and the same handler also never runs on SIGKILL, an OOM kill or a lost node. State that outlives the process, presence in Redis or room membership in a shared store, belongs in TTL or heartbeat storage and cleans itself up; state that dies with the process needs no cleanup. The outcome counts the terminal executions refused or abandoned in `Shutdown::terminal_skipped`, and the transport logs which connections missed their handler. Idle connections close at drain start, which gives most handlers the full window.

**Detached tasks.** The drain tracks executions, not tasks. A background loop holding an `AppHandle`, a queue consumer or a poller, reads `handle.draining()` to stop pulling work when the transports stop accepting, and runs each unit of work through `execute` so the drain waits for it; `execute` answering `Closed` is far too late as a first notice. An app dropped without its shutdown reaching the close step fires `draining()` as the drop releases its servers, and `phase()` reads `Closed`, so such a loop is told to stop there too; a `close` on a kept handle then answers `ShutdownFailure::Dropped` and runs no step (step 6, transports 153).

Without `serve`, a job or CLI on `Connected`, `close` runs the same sequence. There are no sockets to close, and the drain covers live standalone executions: an `execute` still running, or an `Execution` opened with `Execution::open` and not yet dropped. A detached subtask's `handle()` is not one.

`drain_timeout` and `shutdown_timeout` are set on the builder beside the `Timer` (§9.4), and every wait in the sequence is timed by it. `listen()` refuses an app that binds a transport with no `Runtime`, before any server's `prepare`, as `StartupError::Bind` carrying `RuntimeMissing` (§10.2); a runtime is a timer, so an app given `.timer(..)` alone is refused too. A server's `prepare` that panics is caught, as a `close` is, and reported as that server's `ConfigureError`, its `source` a `PreparePanicked` that `is_panic` recognises, handed the `StartupError` or the `ConfigureErrors` it holds; the other servers still prepare, nothing binds, and every server drops when `listen()` returns, which is where a transport lets go of what a `prepare` cut short had taken. The HTTP server keeps each upgrade handler in its list before that handler's `prepare` runs, so its drop releases every handler it prepared, and a hand-off holding the app's handle lets go of the app. Three reasons: a served app with no clock would run a zero-length drain and cut every in-flight request at every shutdown, undetectable until production; it could not enforce the per-call deadlines it accepts; and every transport spawns its connections and calls on the app's runtime. An app that binds nothing, a job or a CLI command, passes `listen()` with the timer alone or with neither. A `close` on `Connected` with no `Timer` runs a zero-length drain, cancelling and abandoning live executions at once, while its hooks run unbounded; §3.9 states the asymmetry.

---

## 10. Errors and diagnostics [44–47]

### 10.1 The wiring pass

`wire()` runs these steps and **collects** errors. It never stops at the first one.

1. **Module graph:** deduplicate identities, refuse a second registration of an identity whose `settings` differ from the first's (`DifferingSettings { module, first, second, settings }`, each side a `ModuleSite`, `settings` each `DifferingSetting { name, first, second }`, with the hint to write the same settings at both or key one, §8.3), refuse a second registration of a `#[module(identity = type)]` module that has fields (`RegisteredTwice { module, first, second }`, decision 73), refuse a setting name one module's settings write twice (`DuplicateSetting { setting, module, first, second }`, `first` the `with` call that recorded the name and `second` the repeat, so an adapter's or an exporter's line prints beside the module's, with the hint to write each name once; the values are not printed), refuse a module importing itself (`ImportCycle`, its path the module's name twice; that import is not also compared for settings), an import cycle through other modules being accepted (§8.2), check that re-exports are visible, refuse an `export::<T>()` of a key the module does not bind (`ExportNotBound`, with the hint to bind it there, or to re-export it with `reexport` when an import provides it), refuse an input declared by a keyed module, and refuse a keyed module keyed again (`NestedKeyed`, naming the module, the outer qualifier its identity carries and the inner one its exports leave under, with the hint to key it once).
2. **Bindings:** find duplicate singles, single/collection mixes, aliases pointing at nothing, values whose `try_value` recorded an `Err` (naming the module and the key; `ModuleDef::try_value` and `Contribute::try_value` alike), a qualified contribution and a single binding under a role key the freeze recorded (§7; the hints "contribute it unqualified" and "contribute it"), and, in tests, overrides that match no binding or more than one, an `in_module::<M>()` over several instances of `M` among them, an override of `dyn Timer` or `dyn Runtime`, refused with the hint "set it with `TestApp::timer(..)`" or "set it with `TestApp::runtime(..)`" (§3.9), a `replace_module` whose original no module imports (`ReplacementUnmatched`, the same stale mock §11 reports for an override), and a second `replace_module` of an original already replaced (`DuplicateReplacement { original, first, second }`, `first` and `second` the two `replace_module` calls, with the hint "keep one; wiring applied only the first"). Over an original nothing imports, the first call is `ReplacementUnmatched` and each later one `DuplicateReplacement`, never both on one call. An input a transport declared (`Transport::inputs`, transports/DESIGN §2.10) beside another transport's declaration of the key with a different seeder set, beside a module's declaration with another seeder, or beside a single binding under the key is `InputConflict { key, first, second }`, each side an `InputOrigin` (§10.2); two transports' declarations of one key whose seeder sets are equal, each written with `also_seeded_by`, merge into one; a binding is reported once per key, and inputs only modules declare keep `DuplicateBinding`.
3. **Visibility:** resolve every injection point against its module's visibility table. Report missing keys (with the injection point, the key and the module) and ambiguous keys (naming every source module). When a missing key's name equals a bound key's name up to a trailing `+ core::marker::Send + core::marker::Sync`, the report names both spellings: `dyn Repo` and `dyn Repo + Send + Sync` are distinct `TypeId`s, and this is the one place the mismatch is visible. Then refuse every read of a controller's key by a binding that is not a controller, through its construction, an optional read included, a hook, its readiness check or an alias naming the controller (`ControllerRead`, naming the controller, the reader, the reader's module and the controller's, with the hint "move the shared logic into a provider both can read"): a controller is the edge of the application, reached by its transport alone. The same refusal covers a module's own hooks and a handler's enhancer closures (the reader is named as `OnModuleInit hook of <module>` or `<controller>::<handler> (enhancer closure)`, with that module). A controller reading a controller, a handler parameter, metadata and a runtime `app.get::<Ctl>()` are not checked.
4. **Dependency cycles:** run a DFS over the resolved edges and print the full path, as in `A → B → C → A`, with the module of each step and the hint to remove one of the reads or to read one through `ModuleRef` where it is used rather than at construction, `get` for one binding and `execute` with `exec.resolver().many()` for a collection, which `get` refuses. A factory closure's parameters and a collection's contributions are edges like any other read; a `ModuleRef` declares no key.
5. **Scopes:** run the needs-execution pass from §6.2, then report scope violations with the path that introduces the execution dependency, and hooks on types that became per-execution. Report a hook, readiness, module-hook or metadata closure that reads `Ext`, `ExecutionRef`, an input or a per-execution key, since no execution exists where it runs (§6.2). Then, for each handler, walk its reachable execution-scoped bindings and report every non-optional input that the handler's transport does not seed (§6.4), with the path from the handler to the service that reads the input, the handler's transport and the input's seeder.
6. **Environment:** check for a `Timer` wherever a wait is written: a readiness `.timeout`, `.attempt_timeout` or `.backoff`, a hook's or constructor's `After(..)`, or a builder knob set on an app with none (§3.9). `Default` and `Unbounded` need none. Each report points at the call that wrote the wait: `.timeout(..)`, `.attempt_timeout(..)` and `.backoff(..)` each record their own `#[track_caller]` location, so a report names that line and not the registration or the `.ready(..)` before it. A trait const's `After(..)`, `CONSTRUCT_TIMEOUT` or a hook trait's `TIMEOUT`, has no call, and its report names the registration that brought the type in. A builder knob set on an app with none names no call. Then refuse `After(Duration::ZERO)` on every item `resolve_bound` resolves, a construction, a readiness check as a whole, one of its attempts and a hook, a module's own included, whatever the app's clock: the item would time out before it starts (`ZeroBound`, naming the item's kind as a `BoundItem`, its binding or `None` for a module's hook, its module and the call that wrote the bound, with the hint `.unbounded()`). On an app with no `Timer` a zero bound is reported as `ZeroBound` alone, since `.unbounded()` answers the missing clock too. `construct_timeout(ZERO)` and `hook_timeout(ZERO)` are refused as `ZeroDefault { knob, at }`, `at` the builder call that set the knob (both methods are `#[track_caller]`), with the hint to write `.unbounded()` on the items; on an app with no `Timer` it is reported beside the knob's `KnobWithoutTimer`, which `.unbounded()` does not answer. A transport's own settings follow the transports' zero rule (transports X22), which servers and links apply in `prepare`; the core resolves none of them.

Steps that depend on a missing piece skip only the affected edges, so one missing binding doesn't hide unrelated errors.

Every type name in a report is a `TypeName` (§3.1), cut to its last path segment, inside generic arguments too: `Arc<PgPool>`, not `alloc::sync::Arc<my_app::db::PgPool>`. A generic transport marker prints the same way, `Q<X>`. Where one report would print two different types alike, `a::Config` and `b::Config`, it prints those types with their full paths, and only those: the report runs `TypeName::colliding` over every name it holds and writes each name in the answer with `{:#}`. A key enters the pass as its type and, when qualified, its qualifier, each on its own. `Store @ a::Replica` beside `Store @ b::Replica` prints `Store` short and the two `Replica`s in full, and `a::Config` beside `b::Config @ Q` prints both `Config`s in full; a key's binding kind plays no part. A transport enters the pass as the `TypeName` of its marker type wherever an entry stores one (§10.2): `InputNotSeeded`'s handler transport and seeder, `ClosureScopeViolation`'s transport, and the transports of `InputConflict`'s two origins. Module names follow the same rule under their own check: `billing::Module` and `users::Module` print in full where one report would print both. A `ModuleName` carries a label and an instance number a `TypeName` does not, which is why the two passes stay separate.

The pass is the report's, not the entry's. `WiringErrors` gathers the names of every entry before formatting, and a type colliding with one in another entry prints in full in both; a `WiringError` or an `InputOrigin` displayed on its own collides its own names. The startup report, `StartupError::Configure`, runs one pass over every failing transport and every type a `PrepareError` names, when the report is built (§10.2). The shutdown report runs one over its hook keys and its transports in `Display`. `StartupError::Bind` names one transport and runs none: a type cannot collide with itself.

What an entry stores decides what the pass reaches. A report entry stores the names it reports as values, a `TypeName`, a `Key` or a `KeyName`, rendered when the report is formatted under the rule above. Descriptive text, a consumer's name such as ``UserService (param `mailer`)``, the steps of a path between services or an item description, may be stored already rendered, with the names inside it written short: it is built while the graph is available and carries module names, field names and positions no `TypeName` holds. A name an entry also stores as a value is never written into its text: it is rendered from the value, and the entry prints it one way. `InputNotSeeded` holds its handler's transport as a value and its `steps` open after the handler, and the headline and the path both write the transport from that value; `ClosureScopeViolation`'s `closure` names the enhancer without its transport, which the report writes after it from the value. Two same-named services inside one dependency path still print alike, a step's type being inside pre-rendered text: that is the rule's scope as it stands.

A sample of the output:

```
error: wiring failed with 5 errors

  × missing dependency `dyn Mailer`
    ├─ needed by UserService (param `mailer`) in UsersModule
    └─ help: import a module that exports `dyn Mailer`, or provide it in UsersModule

  × missing dependency `dyn Repo`
    ├─ needed by ReportService (field `repo`) in ReportsModule
    └─ help: PersistenceModule exports `dyn Repo + Send + Sync`; the injection point reads `dyn Repo`

  × ambiguous dependency `dyn UserRepo` in AppModule
    ├─ exported by PersistenceModule   (src/persistence.rs:14)
    └─ exported by LegacyRepoModule    (src/legacy.rs:9)

  × scope violation: singleton `ReportService` depends on per-execution data
    └─ ReportService → AuditContext (execution) → Ext<CurrentUser>
       help: declare ReportService #[injectable(execution)], or inject a factory

  × input `RequestHead` is seeded by Http, read on a path from the Rpc handler UsersController::get_rpc
    ├─ UsersController::get_rpc (Rpc) → AuditContext (execution) → Dep<RequestHead> (field `head`)
    └─ help: read it as `Option<Dep<RequestHead>>` where the path is shared across transports
```

### 10.2 Error types

```rust
#[non_exhaustive]
pub enum StartupError {
    Wiring(WiringErrors),                                  // everything from wire()
    #[non_exhaustive]
    Connect { error: Box<ConnectError>, teardown: Vec<ShutdownFailure> },   // the walk's failure, and what the destroy hooks run for what had initialized recorded (§9.2)
    Configure(ConfigureErrors),                            // every failure the servers' `prepare` calls reported, before any binds (transports §2.7)
    #[non_exhaustive]
    Bind { transport: TypeName, source: Redacted },        // a transport's own error, or the core's `RuntimeMissing`; one name, printed short
}

/// Every `prepare` failure, one entry per server, collected before any server binds. `Display` writes every entry, and `Debug`
/// the same text. The entries' transports and the names of every `PrepareError` among them enter one `TypeName::colliding` pass
/// when the report is built, before each entry's text is redacted (§10.1).
pub struct ConfigureErrors { errors: Vec<ConfigureError> }

impl ConfigureErrors {
    pub fn iter(&self) -> slice::Iter<'_, ConfigureError>;
    pub fn len(&self) -> usize;
    pub fn is_empty(&self) -> bool;
}

/// One server's configuration failure. `Display` writes the entry as its report does, the transport in full where the report's
/// pass decided so.
#[non_exhaustive]
pub struct ConfigureError { pub transport: TypeName, pub source: Redacted }

/// A `prepare` error whose text names types: what a server answers, boxed, so the startup report decides how each name prints.
/// `text` writes each name with `{:#}` when the set it receives holds it, and with `{}` otherwise; `Display` on a `PrepareError`
/// alone collides its own names.
pub struct PrepareError { names: Vec<TypeName>, entries: Vec<PrepareEntry>, text: Box<dyn Fn(&HashSet<TypeName>) -> String + Send + Sync> }

/// One failure a `PrepareError` lists, as data: the types it names, the source line that declared what failed, what failed
/// (`"pre-dispatch entry"`), the route scopes involved and, for what was refused as something that would never run, why. A
/// test or a tool reads these instead of the report's text.
pub struct PrepareEntry { /* names, location, subject, scopes, reason */ }

impl PrepareEntry {
    pub fn names(&self) -> &[TypeName];
    pub fn location(&self) -> Option<&'static Location<'static>>;
    pub fn subject(&self) -> Option<&str>;
    pub fn scopes(&self) -> &[String];
    pub fn reason(&self) -> Option<Unreached>;
}

/// Why `prepare` refused something as something that would never run; every such refusal carries one. `Display` writes
/// the clause the refusal's text ends with, so the text follows from the reason: "pre-dispatch entry at src/app.rs:9:5 is
/// scoped to `/typo`, which matches no route's path".
#[non_exhaustive]
pub enum Unreached {
    NoRoute,                                // the scope matches no path a route of the table matches, the entry's exclusions taken out
    #[non_exhaustive]
    HealthOnly { service: &'static str },   // gRPC: the only service it matches that the server answers is the health service
                                            // `service`, which runs no scoped entry; `fw-grpc` fills it from tonic's
                                            // `NamedService::NAME`, so the core spells no service name (transports §6.2)
    EmptyScope,                             // HTTP and gRPC: a scoped entry names no route pattern, so it covers no route
    RawGateway,                             // WebSocket: a message handler on a gateway that reads every message raw, one
                                            // implementing `Gateway` by hand, so the handler would never run (transports §4.1)
    BackslashSegment,                       // HTTP: a route, scope, exclusion or upgrade path whose static segment holds a `\`, raw
                                            // or as `%5C`, which no request path reaching it holds (transports §3.2)
}

impl PrepareError {
    pub fn new(names: impl IntoIterator<Item = TypeName>, text: impl Fn(&HashSet<TypeName>) -> String + Send + Sync + 'static) -> Self;
    pub fn with_entries(self, entries: Vec<PrepareEntry>) -> Self;
    pub fn names(&self) -> &[TypeName];
    pub fn entries(&self) -> &[PrepareEntry];                  // one per failure the server found, in order
    pub fn text(&self, full: &HashSet<TypeName>) -> String;   // each name in `full` written with its full path
}

/// `listen()`'s refusal of an app that binds a transport with no `Runtime` (§9.5), `.timer(..)` alone included, stored in
/// `StartupError::Bind`'s `source` like a transport's error; `source.downcast_ref::<RuntimeMissing>()` tells it from a port
/// already taken. `Display` names the transport and `.runtime(..)`, which sets the timer too. `AppHandle::mounted` answers it
/// for the same app.
#[non_exhaustive]
pub struct RuntimeMissing { pub transport: TypeName }

/// A failure in the connect phase, the `error` of `StartupError::Connect` from `connect` and of `LoadError::Connect` from `load`.
#[non_exhaustive]
pub enum ConnectError {
    #[non_exhaustive]
    Construct { key: KeyName, module: ModuleName, reason: FailureReason },
    #[non_exhaustive]
    Readiness { key: KeyName, module: ModuleName, attempts: u32, reason: FailureReason },   // `module` is the checked binding's module, so two keyed modules of one type read apart; `reason` is how the last attempt ended, `TimedOut { limit: Item }` for the whole check, or `Errored` holding a `Permanent` `RuntimeShutDown` for a backoff or a bound the runtime refused (§9.3)
    #[non_exhaustive]
    Hook { hook: HookKind, key: KeyName, reason: FailureReason },
}

/// Why a hook, a construction, a readiness check or a transport's `close` did not complete. Held as a field on `ConnectError`
/// and `ShutdownFailure::Close`, and boxed on `ShutdownFailure::Hook` and `LookupError::Construct`; implements `Display` and
/// `Debug`, not `Error`, so no `BoxError` holds one.
#[non_exhaustive]
pub enum FailureReason {
    Panicked(Redacted),                            // the payload, converted to a message
    #[non_exhaustive]
    TimedOut { after: Duration, limit: Limit },    // `after` is the configured duration of the limit that fired
    Skipped,                                       // never started: `shutdown_timeout` had expired (§9.5); shutdown only
    Errored(Redacted),                             // returned `Err`: a construction, an init or bootstrap hook, the last attempt of a readiness check,
                                                   // or a transport's `close`; or a closure hook whose parameter read failed, the `LookupError` by `downcast_ref` (§9.1);
                                                   // or a bound the `Timer` refused, a `Permanent` holding a `RuntimeShutDown` (§3.9): a hook's, a construction's,
                                                   // a readiness check's, the shutdown cap or a `close` bound
}

/// Which limit a `TimedOut` hit.
#[non_exhaustive]
pub enum Limit {
    Item,          // an explicit bound on the whole item: a trait const's `After(..)`, or `.timeout(..)` on a binding handle (§3.9)
    Attempt,       // an explicit `.attempt_timeout(..)` on a readiness check (§9.3)
    Default,       // the app default for this kind of item: `hook_timeout` for a hook and for a transport's `close` under no cap, `construct_timeout` for a construction and for one attempt of a readiness check
    ShutdownCap,   // `shutdown_timeout` cutting a hook or a transport's `close` mid-run (§9.5)
}

/// An error from outside the core, after the redaction function (§9.3). Formatting writes the redacted text and never the
/// original; `source()` is `None`, so a reporter walking the chain stops at the text too. The original is reached through
/// the two methods alone.
pub struct Redacted { /* one `Box` holding the original, a `BoxError`, and the redacted text */ }

impl Display for Redacted { /* writes the text */ }
impl Debug   for Redacted { /* writes the text; never formats the original */ }
impl Error   for Redacted { /* `source()` returns `None` */ }

impl Redacted {
    pub fn downcast_ref<E: Error + 'static>(&self) -> Option<&E>;   // the original, by type, a shared one included (§3.8)
    pub fn original(&self) -> &(dyn Error + 'static);               // the original itself, a shared one included, for a walk of its chain
}

/// The outcome of a shutdown, received by `serve` and by every `close` caller (§9.5).
#[non_exhaustive]
pub struct Shutdown {
    pub signal: Signal,            // the trigger that won
    pub abandoned: usize,          // executions still alive at the drain's end, `drain_timeout` or the earlier `shutdown_timeout` expiry, with no committed answer (§9.5)
    pub terminal_skipped: usize,   // terminal executions refused or abandoned (§9.5)
}

#[derive(Clone)]                                                  // one outcome, several receivers; the failures sit behind the Arc
#[non_exhaustive]
pub struct ShutdownError { pub report: Shutdown, pub failures: Arc<[ShutdownFailure]> }
// `Display` runs one `TypeName::colliding` pass over every failure's names, each hook key's type and qualifier and each
// `Close`'s transport, and writes each failure against it; a `ShutdownFailure` displayed alone collides its own names (§10.1).

#[non_exhaustive]
pub enum ShutdownFailure {
    #[non_exhaustive]
    Hook { hook: HookKind, key: KeyName, reason: Box<FailureReason> },   // `Errored` only for a closure hook whose parameter read failed: the hook traits return `()`
    #[non_exhaustive]
    Close { transport: TypeName, reason: FailureReason },          // `Errored` holding the transport's own error, `Panicked` for a `close` that panics, `TimedOut` for one exceeding its bound, `Errored` holding a `Permanent` `RuntimeShutDown` for a bound the `Timer` refused; never `Skipped` (§9.5)
    Dropped,                                                        // a `close` called once a drop had released the servers before the close step ended; it runs no step (§9.5)
    #[non_exhaustive]
    Serve { transport: TypeName, end: ServeEnd },                   // the transport's `serve` ended while the app ran and started this shutdown; listed first (§9.5)
}

#[non_exhaustive]
pub enum ServeEnd {
    Errored(Redacted),   // `serve` returned `Err`: the transport's own error, redacted
    Panicked(Redacted),  // `serve` panicked while polled: the payload as a message, redacted
    Returned,            // `serve` returned `Ok` before the shutdown began
}

/// `execute` and `Execution::open` from Draining on; `load` carries it in `LoadError::Closed` from Stopping on (§9.5).
#[non_exhaustive]
pub struct Closed;

/// Why `load` did not return a `ModuleRef` (§8.6).
#[non_exhaustive]
pub enum LoadError {
    Closed(Closed),
    Wiring(WiringErrors),      // the module wired against the frozen graph, every error collected
    #[non_exhaustive]
    Connect { error: Box<ConnectError>, teardown: Vec<ShutdownFailure> },   // a construction, readiness check or init hook of the module failed, and what its teardown recorded
    Refused(LoadRefusal),
}

/// What a lazily loaded module may not bring, because the graph has been handed out (§8.6).
#[non_exhaustive]
pub enum LoadRefusal {
    #[non_exhaustive]
    Controllers { module: ModuleName },
    Middleware { module: ModuleName },
    Contribution { module: ModuleName, key: KeyName },   // to a collection the module does not introduce
    GlobalExport { module: ModuleName, key: KeyName },
    Input { module: ModuleName, key: KeyName },          // inputs belong to transports; the per-handler check ran at wiring time (§6.4)
}

/// One wiring failure, a variant per failure of §10.1, held by `WiringErrors`. The two variants that name a transport hold it
/// as a `TypeName` and write it when the report is formatted; the entry's descriptive text carries no transport.
#[non_exhaustive]
pub enum WiringError {
    ..,
    /// Step 5: an enhancer a handler declares by closure in an explicit singleton scope, which reads per-execution data.
    /// `closure` names it without its transport, as in ``method-level guard #2 of UsersController::get``, and the report
    /// writes `transport` after it, as in `(Http)`; `role` is `guard`, `interceptor` or `error handler`; `path` runs from
    /// the injection point to the read of execution data.
    #[non_exhaustive]
    ClosureScopeViolation { closure: String, transport: TypeName, role: &'static str, path: Vec<String>, at: &'static Location<'static> },
    /// Step 5: a non-optional input read on a path from a handler whose transport does not seed it. `steps` holds the steps
    /// after the handler, each binding between it and the read and then the injection point; the report writes
    /// `{handler} ({transport})` ahead of them, the transport from the value, and names `seeder` in the headline.
    #[non_exhaustive]
    InputNotSeeded { handler: String, transport: TypeName, input: KeyName, seeder: TypeName, steps: Vec<String> },
    /// Step 1: a keyed module keyed again; the identity carries `outer` and the exports `inner`.
    #[non_exhaustive]
    NestedKeyed { module: ModuleName, outer: TypeName, inner: TypeName },
    /// Step 1: a second registration of an identity whose `Module::settings` differ from the first's; `first` holds the
    /// identity. `settings` lists each setting that differs, in the first's order. `load` answers it too (§8.6).
    #[non_exhaustive]
    DifferingSettings { module: ModuleName, first: ModuleSite, second: ModuleSite, settings: Vec<DifferingSetting> },
    /// Step 1: a second registration of a `#[module(identity = type)]` module that has fields (decision 73). Hint: give it an
    /// identity to share it.
    #[non_exhaustive]
    RegisteredTwice { module: ModuleName, first: ModuleSite, second: ModuleSite },
    /// Step 1: a setting name written twice in one module's `Module::settings`; `first` is the `ModuleSettings::with`
    /// call that recorded it, `second` the repeat. Hint: write each name once.
    #[non_exhaustive]
    DuplicateSetting { setting: &'static str, module: ModuleName, first: &'static Location<'static>, second: &'static Location<'static> },
    /// Step 3: a binding that is not a controller reads a controller's key. `consumer` names the reader as `Missing` does.
    #[non_exhaustive]
    ControllerRead { controller: KeyName, consumer: String, module: ModuleName, controller_module: ModuleName },
    /// Step 3: a read declared with `Requirement::dep_imported` of a key the reading module cannot see; `import` is the module
    /// whose import binds it, as the application writes it. Hint: add `import` to a module's `imports`. A WebSocket gateway on
    /// the HTTP server's port declares one naming `ulo_ws::WsModule::for_root()` (transports decision 182).
    #[non_exhaustive]
    NotImported { key: KeyName, consumer: String, module: ModuleName, import: &'static str },
    /// Step 2: a controller registered as a provider, where its handlers would never mount. Known when the graph also
    /// registers the type with `ModuleDef::controller`, in any module, whatever binds it as a provider (`controller_at`
    /// names that module and line), or when its `Construct::dependencies` marks it (`Dependencies::mark`, which the
    /// macros' impls call) and it is registered through `provide`, `provide_with`, `try_provide_with` or
    /// `Contribute::provide` (`controller_at` is `None`); one entry per provider registration (§14, item 28). `key` is a
    /// contribution's collection. Hint: remove the provider registration, or, with no controller registration,
    /// `m.controller::<C>()`.
    #[non_exhaustive]
    ControllerProvided {
        key: KeyName,
        controller: TypeName,
        module: ModuleName,
        at: &'static Location<'static>,
        controller_at: Option<(ModuleName, &'static Location<'static>)>,
    },
    /// Step 6: `After(ZERO)` on a construction, a readiness check, one of its attempts or a hook. `binding` is `None` for a
    /// module's own hook; `at` is the call that wrote the bound, or the registration for a trait const. Hint: `.unbounded()`.
    #[non_exhaustive]
    ZeroBound { item: BoundItem, binding: Option<KeyName>, module: ModuleName, at: &'static Location<'static> },
    /// Step 6: `construct_timeout` or `hook_timeout` set to zero; `at` is the builder call that set it. Hint: `.unbounded()` on the items.
    #[non_exhaustive]
    ZeroDefault { knob: &'static str, at: &'static Location<'static> },
    ..
}

/// Where a module was registered: the app's root, `ModuleDef::import` in `importer` at `at`, or the `AppHandle::load` call at
/// `at`.
#[non_exhaustive]
pub enum ModuleSite { Root, #[non_exhaustive] Import { importer: ModuleName, at: &'static Location<'static> }, #[non_exhaustive] Load { at: &'static Location<'static> } }

/// One setting two registrations disagree on, each value as `ModuleSettings` wrote it, `None` where that side lacks it. The fields
/// hold the values unredacted; its `Debug`, and the entry's `Display` and `Debug`, print them after the graph's redaction function.
#[non_exhaustive]
pub struct DifferingSetting { pub name: &'static str, pub first: Option<String>, pub second: Option<String>, /* private: the redacted texts */ }

/// The item a bound is written on: `Construction`, `ReadinessWhole`, `ReadinessAttempt` or `Hook(HookKind)`.
#[non_exhaustive]
pub enum BoundItem { Construction, ReadinessWhole, ReadinessAttempt, Hook(HookKind) }

/// One side of the `WiringErrors` entry `InputConflict { key, first, second }` (§10.1 step 2): where an input declaration,
/// or the binding it collides with, was written. `at` is the call's `#[track_caller]` location, as on every binding (§2).
/// `Display` writes the line the report prints, the transports short unless two different ones on the line print alike.
#[non_exhaustive]
pub enum InputOrigin {
    #[non_exhaustive]
    Transport { name: TypeName, at: &'static Location<'static> },                             // `Transport::inputs`; `name` is the marker type
    #[non_exhaustive]
    Module { module: ModuleName, seeders: Vec<TypeName>, at: &'static Location<'static> },    // `m.input::<T>().seeded_by::<Tr>()`, each `Tr`
    #[non_exhaustive]
    Binding { module: ModuleName, at: &'static Location<'static> },                           // a single binding under the key
}

#[non_exhaustive]
pub enum LookupError {                                           // [47]
    #[non_exhaustive]
    NotFound { key: KeyName, kind: LookupKind },                  // binding, extension, input
    #[non_exhaustive]
    NotReady { key: KeyName },                                    // a singleton the connect walk has not built yet (§8.5)
    #[non_exhaustive]
    WrongType { key: KeyName, requested: &'static str },          // an erased `Key` asked for a `T` it does not hold
    #[non_exhaustive]
    WrongKind { key: KeyName, expected: BindingKind, found: BindingKind },   // single read as collection, or the reverse
    #[non_exhaustive]
    ExecutionRequired { key: KeyName },
    #[non_exhaustive]
    Ambiguous { key: KeyName, sources: Vec<ModuleName> },         // a runtime lookup of a key two visible exports answer (§8.2)
    #[non_exhaustive]
    AmbiguousModule { module: &'static str, candidates: Vec<ModuleName> },   // `app.module::<M>()` over several instances of `M` (§8.5)
    #[non_exhaustive]
    Construct { key: KeyName, reason: Box<FailureReason> },       // a build inside a call failed, panicked or timed out (§3.4)
    #[non_exhaustive]
    Closed { key: KeyName },                                      // a singleton lookup from Destroying on (§9.5)
    #[non_exhaustive]
    Reentrant { key: KeyName, reader: Box<KeyName> },                  // `key`, execution-scoped or transient, read through a `ModuleRef` or `ExecutionRef` while it is being built further up the reads; `reader` read it back (§8.5)
}

/// What `Construct::construct` fails with (§3.4). `Dependency` is a dependency's own `LookupError`, reported unchanged;
/// `Failed` is the constructor's own error, redacted and reported as `FailureReason::Errored`. A panic or a timeout is
/// no variant: a constructor cannot report either about itself, and the core records both from outside the poll.
#[non_exhaustive]
pub enum ConstructError {
    Dependency(LookupError),   // `?` on a dependency read, through `From`
    Failed(BoxError),    // `ConstructError::failed(e)`, or the macro's mapping of a constructor's `Result<Self, E>`
}

impl From<LookupError> for ConstructError { /* `Dependency` */ }   // no blanket `From<E: Error>`: it would overlap this impl (§3.4)

impl ConstructError {
    pub fn failed(e: impl Into<BoxError>) -> Self;   // `Failed`
}

/// A guard's `Ok(false)`, as the error handlers see it (§7).
#[non_exhaustive]
pub struct GuardRejected { pub guard: &'static str }

/// A panic in one stage of the pipeline, caught at that stage (§7): the interceptors around a panicking handler receive it
/// from `next.run()`, and the error handlers receive any stage's.
#[non_exhaustive]
pub struct PanicRecovered { pub stage: DispatchStage, pub message: Redacted }   // the payload, converted to a message

/// Which stage of `dispatch` panicked.
#[non_exhaustive]
pub enum DispatchStage { Guard, Interceptor, Handler, ErrorHandler }

/// A server's `prepare` that panicked, caught by `listen()` and stored as that server's `ConfigureError`'s `source`.
#[non_exhaustive]
pub struct PreparePanicked { pub message: Redacted }

/// `true` wherever a core error carries a caught panic: a `PanicRecovered`, a `LookupError::Construct { reason: Panicked(..) }`,
/// the two shapes a panic during a call takes (§7), a `PreparePanicked`, bare or in an entry of `StartupError::Configure` or of the
/// `ConfigureErrors` it holds, and a `ConnectError` whose reason is `Panicked`, bare or as the
/// `error` of `LoadError::Connect` or `StartupError::Connect`, their `teardown` not read, which a handler propagating a `load` inside a call hands over (§8.6).
/// It follows a wrapper's chain to 32 levels: `Errored`'s `Redacted`, `ConstructError::Failed` and every `source()`, a
/// `Redacted`'s through its original's chain, as `is_permanent` follows it to a `Permanent` (§14, item 70).
pub fn is_panic(error: &BoxError) -> bool;
```

`is_panic` takes `&BoxError` because that is what a caller holds: an error handler receives `err: BoxError`, an interceptor gets one from `next.run()`, and `is_panic(&err)` compiles against both. Against a `dyn` parameter, `&(dyn Error + 'static)` or `&(dyn Error + Send + Sync + 'static)` alike, `is_panic(&err)` is E0277: the coercion commits to unsizing the `Box` itself, and `Box<dyn Error>` does not implement `Error`; the caller would have to write `&*err`. The cost is that a caller holding a bare `&dyn Error`, a `source()` link, cannot pass it; `is_panic` walks `source()` itself. Nesting is rare: a dependency's `LookupError` passes up unchanged through `ConstructError::Dependency`, so a panic any depth down a dependency chain arrives as the top-level `LookupError::Construct { reason: Panicked(..) }` naming the deepest key. The nesting that does occur is a constructor or a `try_` factory returning a dependency's `LookupError` as its own `Err`, which arrives as `Construct { reason: Errored(Redacted(LookupError::Construct { reason: Panicked(..) })) }`, and `is_panic` reaches it. Inside a `Redacted` it walks the original through `original`, as `is_permanent` does: the core's own types by their structure, `PanicRecovered`, `PreparePanicked`, `LookupError`, `ConstructError`, `ConnectError`, `LoadError`, `StartupError`, `ConfigureErrors`, `ShutdownError`, `Redacted` and a panic's own message, the `Redacted` a `TaskEnd::Panicked` holds, and any other error down its own `source()` chain. So an error keeping a task's panic as its `source()`, as a link answers a call whose connect task panicked (transports §5.2), is found, and so is a user's error type wrapping a panicked `LookupError`, returned as a constructor's `Err` and redacted. The answer is a boolean, which leaks none of the original's text.

`StartupError::Configure` is the startup report, one `ConfigureError` per server whose `prepare` failed (transports §2.7). A server whose failure names types answers a `PrepareError`, boxed, as its `prepare` error: the names, and a closure writing the text given the set of names to write in full. `listen()` holds every server's error before it builds the report. It downcasts each to `PrepareError`, runs `TypeName::colliding` once over every failing transport together with the names of every `PrepareError`, and writes each `PrepareError`'s text against that set. The text is then redacted against the graph's secrets and stored as the entry's `source`, which is why the pass runs when the report is built and not in `Display`, where no registry exists. A server answering any other error keeps its text, which contributes no names, and its transport still enters the pass. Each `ConfigureError` records whether the pass wrote its transport in full, and an entry displayed alone prints as the report does; `source.downcast_ref::<PrepareError>()` reaches the original, whose own `Display` collides its own names. `ShutdownError` holds no text written ahead of time, and its reasons are redacted when recorded, so its pass runs in `Display`.

`WiringErrors` carries one entry per failure from §10.1, the `Err` a `try_value` recorded among them, held as a `Redacted` (§9.3). `WrongType` is the one failure a caller reaches deliberately, through `Resolver::by_key::<T>(key)` (§3.1), where an erased key does not fix `T`. A typed read answers it too, `dep`, `many`, an entry's `resolve` or an input read, when the stored instance does not hold the key's type after the recorded coercion; with consistent records that cannot happen, and the alternative to the variant is a panic.

Every public error type is `#[non_exhaustive]`, the seven structs included, and so is every struct variant of every public error enum, in every crate, and of every public enum an error holds, reached by any path, `FailureReason`, `ShutdownFailure`, `LoadRefusal`, `WiringError`, `InputOrigin`, `ModuleSite` and `Unreached` among them, the transport crates' `ExtractError`, `MetadataError`, `TlsError`, `CloseCodeError`, `Detail` and `Frame` too (§14, item 71): a variant added to an enum breaks no caller that matches on it, a field added to `Shutdown`, `ShutdownError`, `ConfigureError`, `GuardRejected`, `PanicRecovered` or `RuntimeMissing`, or to a variant such as `StartupError::Bind` or `ConnectError::Readiness`, breaks no caller that destructures one, and `Closed` is constructed by the core alone. `Redacted`, `ConfigureErrors` and `PrepareError` carry no attribute: their fields are private, which closes them the same way. Code outside the core reads these types and builds none of them but `ConstructError`, which a constructor returns, `PrepareError`, which a server's `prepare` returns, with the `Unreached::HealthOnly` reason `ulo-grpc` gives an entry of one, through `Unreached::health_only`. `ConnectError` is its own type because `StartupError` and `LoadError` share only the connect phase. `load` reports wiring errors through its own `Wiring` and binds no transport; a `LoadError` wrapping a whole `StartupError` would carry two cases that can be constructed and never occur. Composed from exact parts, every variant of both is reachable, and code handling a construction failure handles it once for startup and load alike.

Every public error type is under 128 bytes, the size at which clippy's `result_large_err` fires on a `Result` holding it as its `Err`, so a caller's `fn start() -> Result<(), StartupError>` passes the lint without an `allow`. Three types are never an `Err` unboxed and carry no size pin. `ConnectError`, 136 bytes, is boxed in `StartupError::Connect` and `LoadError::Connect` and returned boxed by the core's own functions. The GraphQL bindings' `GqlResponse`, 176 bytes under `--all-features`, where `bson` turns on `serde_json/preserve_order` and a `Value` takes 72 bytes, is boxed at each private function that fails with it; `GqlError`, 144 bytes there, is an entry of a `GqlResponse`'s error list and no function's `Err`. A feature another crate in the build enables grows a type past what the workspace measures without it, so a pin on either would hold one feature set and not the user's. A `const` assertion beside each type's definition fails the build when a field takes it over the line (§14, item 67). The payload that would cross it is boxed: the whole `ConnectError` in both `Connect` variants, which leaves `StartupError` at 48 bytes and `LoadError` at 112, the `FailureReason` in `LookupError::Construct` and `ShutdownFailure::Hook`, and a `Redacted`'s original and text in one allocation, so an error carrying one holds a pointer. Stable Rust has no pattern through a `Box`: a caller binds the field and matches its contents, `Connect { error, .. }` and then `match *error`.

Neither the core nor the macros panic or exit [45]. Panics inside user constructors, factories, readiness checks and hooks are caught at the poll boundary and reported as `FailureReason::Panicked` on `ConnectError::Construct`, `ConnectError::Readiness`, `ConnectError::Hook` or `ShutdownFailure::Hook`; a transport's `close` is caught the same way and reported on `ShutdownFailure::Close` (§9.5). A panic inside a call's pipeline, in a guard, an interceptor, the handler or an error handler, is caught at that stage and travels as `PanicRecovered`, through the interceptors around a handler and then to the error handlers (§7); one while the container builds an enhancer or the controller inside a call is `LookupError::Construct { reason: Panicked(..) }`, and `is_panic` recognises both, and a `ConnectError` carrying one. That holds unless the binary is built with `panic = "abort"`, where nothing can be caught. `FailureReason` is one enum for every place a hook, a construction, a readiness check or a transport's `close` can fail, so a timeout reads the same on an init hook, a destroy hook, a constructor, a check and a `close`; `Skipped` is reachable at shutdown alone, on `Hook` alone, nothing capping startup as a whole and every `close` starting. It is a field everywhere it appears, boxed on `LookupError::Construct` and `ShutdownFailure::Hook` (§14, item 67), and implements no `Error`: `BoxError::from(reason)` does not compile. `TimedOut` names the limit that fired and carries that limit's configured duration: an item's own bound and the cap are often the same round number, and the duration alone would not tell them apart. A hook or a `close` still running when the cap expires reports `ShutdownCap`, whatever its own bound was.

`Readiness` names the key and the module that owns the checked binding, as `Construct` does, so two keyed modules of one type that both fail (`StoreModule<Sqlite> @ Primary` and `@ Replica`) read apart; it keeps `attempts`, and its `reason` is how the check ended, by the rules in §9.3. `Limit::Default` has one meaning, the app default for the item's kind, and `after` carries that default's duration: `construct_timeout` on a check's attempt or a construction, `hook_timeout` on a hook.

The core consumes a `ConstructError` by variant. Inside a call, a `Dependency` is reported as the `LookupError` it carries, on the path that error was already taking. During `connect`, where `ConnectError` carries no `LookupError`, a `Dependency` holding a nested build's `Construct { key, reason }` becomes `ConnectError::Construct` naming that deeper key, and one holding any other lookup error, the `NotReady` a constructor's `ModuleRef::get` meets, becomes `ConnectError::Construct` naming the binding itself with `reason: Errored(..)`, the `LookupError` by `downcast_ref`. A `Failed` is reported as `Construct { reason: Errored(..) }`, on `ConnectError` during `connect` and on `LookupError` inside a call. `Failed` holds the error as the constructor returned it, and the redaction function (§9.3) runs when the core stores it as `Errored`. It runs on `Failed` alone: a `LookupError` is the core's own, and any outside error inside it was redacted where it was stored.

Every error the core did not create itself, from user code, integrations or transports, passes through the redaction function before it is stored in any core error type (§9.3), and the field that stores it is a `Redacted`: `FailureReason::{Errored, Panicked}`, a transport's `close` error among the former, the `source` of `StartupError::Bind`, the `source` of each `ConfigureError`, and the `try_value` entry in `WiringErrors`. A bind error rarely carries a credential, but a TLS key path or a proxy URL with a password in it can, and the cost is one call on a path that fails once. `Bind` also carries the one error the core writes itself on that path, `listen()`'s refusal of an app with a transport and no `Runtime` (§9.5): a `RuntimeMissing { transport }`, wrapped in the same `Redacted` to keep the field one type, and a struct rather than a message, which lets `downcast_ref::<RuntimeMissing>()` tell a misconfigured app from a port already taken. The type is kept because the original has to stay reachable on the runtime path: `LookupError::Construct` fires inside a call, and an execution-scoped constructor failing with a domain error, a tenant not found, is mapped to a 404 by an error handler that downcasts the `LookupError`, then the `reason`'s `Redacted` to the domain type. With the original replaced by its text that mapping would be impossible. What the type enforces is that no formatting prints the original: `Display` and `Debug` write the text, and `source()` is `None`, which is what keeps an error-chain reporter, one that walks `source()` and prints every link, from printing the original and bypassing the redaction. The original is reached through `downcast_ref` and `original` and nowhere else, two methods a reviewer can find. Both borrow, so a shared original (§3.8) needs no type of the core's to hand out, and `original` hands the original over for a walk of its own `source()` chain, which a `Redacted`'s own `source()` stops short of.

`Closed` is a refusal by phase (§9.5): `execute` and `Execution::open` answer it from Draining on, `load` from Stopping on inside `LoadError`, which holds `load`'s other failures beside it; `StartupError` never carries `Closed`. The lookup form is `LookupError::Closed`, from the first destroy hook on, so an abandoned execution is never handed an instance whose destroy hook has run; one it already holds stays a valid object.

### 10.3 Compile-time diagnostics [46]

Compile-time diagnostics use two mechanisms:
- **Trait-bound errors** carry `#[diagnostic::on_unimplemented]` messages written as instructions ("write `Dep<PgPool>`", "implement `Guard<Http>`", "remove the scope argument").
- **Macro errors** use `syn::Error` spans on the exact token (two `#[construct]` functions, no constructor and no `new`, a provider in `#[module]` that doesn't implement `Construct` together with the hint "add #[injectable] or bind it with a factory").

The macro emits one assertion per injection point with `quote_spanned!`, so errors point at the offending field or parameter rather than at the macro invocation.

---

## 11. Testing [48]

```rust
let app = TestApp::of(AppModule)
    .override_value::<dyn UserRepo>(Arc::new(InMemoryRepo::default()))
    .override_factory(|| async move { FixedClock::at(t0) })
    .override_try_factory(|| async { TestBus::connect().await })
    .override_value::<AuditLog>(Arc::new(NullAudit)).in_module::<BillingModule>()
    .override_value::<PgPool>(Arc::new(replica_pool)).in_module_keyed::<DbModule, Replica>()
    .replace_module(MailModule, FakeMailModule)
    .connect()
    .await?;

let users = app.get::<UserService>().await?;
```

Override rules:
- An override replaces the recipe of an existing key and keeps its origin module, its visibility and its exports, so production modules stay untouched. The replaced binding's readiness check and lifecycle hooks, closure hooks and its type's own alike, belong to the implementation it replaces and are dropped with it; the bindings that implementation read stay in the graph and are built as before, since another binding may read them. An override of an `also_as` key splits it off, and the original binding keeps its check and hooks (§14, 50).
- An override names no module by default and must match exactly one binding. One that matches none is a wiring error, which catches stale mocks after refactors. One that matches several, two modules each binding the key privately, is a wiring error listing every match; `.everywhere()` replaces all of them explicitly. Replacing several by default would make a test pass for the wrong reason.
- `.in_module::<M>()` scopes an override to one module. Over several instances of `M`, configured or keyed, it is itself ambiguous and fails as one; `.in_module_keyed::<M, Q>()` and `.in_module_of(&config)` name one instance.
- An override targets `T @ ()`. A binding written `.qualified::<Q>()` is reached by the same method on the pending override, `.override_value::<PgPool>(fake).qualified::<Replica>().in_module::<DbModule>()`, one spelling for every override kind, in the order the binding it replaces was written. `TestApp` carries a state: `Settled` between overrides, `Pending` after a single override, where `.qualified`, `.in_module*` and `.everywhere()` describe it. `.qualified` answers `TestApp<Pending>`, so `.in_module*` and `.everywhere()` still follow, and a second `.qualified` replaces the first. Inside a keyed module the binding's own key is unqualified, and `in_module_keyed::<M, Q>()` reaches it as written.
- An override can't change a key's kind (single or collection). `override_many` replaces an entire collection and answers `TestApp<PendingMany>`: its `.qualified::<Q>()` targets the collection `T @ Q` and answers `TestApp<Settled>`, and it has no `.in_module*`, a collection being app-wide (§8.2). Every method the states share stays reachable: `override_many(..).connect()` reads as before.
- `override_factory` and `override_try_factory` follow §4's split: the plain one binds the future's output, the `try_` one its `Ok` type.
- `replace_module` swaps by identity and removes the original's whole subgraph: its bindings with their checks and hooks, its own hooks, and every module only it imports. It is the form for a fake that must not reach what the original's dependencies dial. The original's `register` still runs once, into a node the wiring drops, to read its exports. The replacement must export a superset of the original's keys, or wiring reports what's missing. One whose original no module imports is a wiring error, `ReplacementUnmatched`, the same stale mock an override that matches nothing is. A second `replace_module` of an original already replaced is a wiring error, `DuplicateReplacement`, naming both calls.
- The `Timer` and the `Runtime` are app configuration, not bindings to override. `TestApp::timer(..)` sets the one clock that times the drain, the hooks, the constructions and the readiness checks and that services read as `Dep<dyn Timer>` (§3.9); `TestApp::runtime(..)` sets the clock and the executor as one object, read as `Dep<dyn Runtime>` and as `Dep<dyn Timer>`, which an app under test that binds a transport needs (§9.5); `TestApp::timer(..)` after `TestApp::runtime(..)` replaces only the clock, so a test runs a real executor under its own clock, `Dep<dyn Runtime>` sleeping on that clock too (§9.4). `override_value::<dyn Timer>` is a wiring error with the hint "set it with `TestApp::timer(..)`", and `override_value::<dyn Runtime>` one with the hint "set it with `TestApp::runtime(..)`".
- **`ulo::testing::ScriptedTimer` is the clock for a test of an app with servers.** It is a `Timer` no sleep of which ends until the test calls `advance`; it counts the live sleeps by length (`live`), records the clock time at which a sleep last answered `Pending` (`pending_at`), and answers a named length as a runtime that has shut down does (`refuse`, `refused`). It needs no runtime and no cargo feature, so a user's test names it with `ulo` as the only dependency, and the conformance support crate re-exports the same type. A paused tokio clock (`tokio::time::pause`) is the wrong tool for such an app: it advances on its own whenever the runtime has nothing else to run, and a server waiting on a request in flight has only its own timers, so the clock jumps to them; a request held across the drain window measured 28 to 61 s of clock time in examples 10 and 13. The scripted clock moves only where the test says. The module documentation of `ulo::testing` carries a compiled doctest.
- **`ulo::testing::TestServer` serves a `TestApp` over a socket.** `TestServer::start_on(app, servers)` wires and connects the app, binds any `ulo::Server` or tuple of servers (an HTTP, WebSocket or RPC one, so a test of a non-HTTP app names no HTTP crate) and serves them in a task of the app's runtime. The core binds nothing and names no runtime: the runtime is the one the `TestApp` was given, and the listener is the one the server names. `ulo_runtime_tokio::testing::start(app, servers)` and `ulo_runtime_smol::testing::start(app, servers)` mirror `run` by setting the app's runtime to the current tokio runtime or the smol runtime whose `block_on` the test is inside, so they replace a clock set before them with `TestApp::timer`; a test that pins a clock writes `.runtime(r).timer(clock)` and calls `start_on`. `ulo_http_hyper::testing::TestServerExt::start(app)` binds `ulo_http_hyper::Server` on `127.0.0.1:0` behind the backend's default `tokio` feature. `addresses()` is every address bound, none for a broker link, `addr()` the first, a panic when there is none, `url(path)` the `http://` URL of a path, `handle()` the app's handle. `close()` closes under the signal `test` and answers the `Shutdown` `serve` reported, the port released when it returns; a drop without `close` aborts the serving task, runs no shutdown, and releases the port when the runtime next runs that task.

---

## 12. What is refused, and where

| Refusal | When | Mechanism |
|---|---|---|
| A field or parameter whose type is not `FromContainer` | compile | `FromContainer` bound + `on_unimplemented` |
| `Dep<T>` with `T` not `Send + Sync` | compile | bound on `Dep`; the auto trait's own message, no hint |
| `Ext` / `ExecutionRef` in an explicit singleton | compile | `AllowedIn<S>` |
| A hook on an explicit execution-scoped or transient type | compile | `Construct<Scope: HookCapable>` |
| A closure hook or `.ready(..)` on an execution-scoped or transient handle | compile | typed binding handle (§9.1) |
| `.qualified::<Q>()` on an enhancer contribution | compile | `Contribute<.., Enhancer>` has no `qualified`; a role collection is read unqualified (§7) |
| `enhancer::<K>()` with a `K` that is not a role key | compile | `Role` bound + `on_unimplemented` (§3.7) |
| `StoreModule::migrate` on a store implementing no `MigrationLock` | compile | method bound + `on_unimplemented` naming the stores that implement it (integrations §4) |
| A backend or host asked for a capability it is declared without (`upgrades`, `h2c`, `peer_addr`, `forward_miss`, `host_extensions`, ...), or an RPC handler of a shape or payload the link does not carry | startup (`prepare`) | the `Server`'s `prepare` against `BackendLimits`, `EmbedLimits` or `Capabilities`, the refusal naming the capability and, for a boolean capability, the reason the declaration carries; a call shape outside `Capabilities::shapes` is named without one, and a binary payload on a link reporting `binary: false` is named with the codec that carries it, `.codec(Codec::Cbor)` (transports §3.7, §3.8, §5.3) |
| A second bound on one binding-handle item: `.timeout` twice, `.unbounded()` beside `.timeout` or `.attempt_timeout`, either after `.unbounded()` | compile | typed binding handle: the item's bounded state has neither method (§9.1) |
| An enhancer that lacks its role trait, including a controller-level one for any handler's transport | compile | `Guard<T>` and similar bounds, one per handler, naming the handler |
| A constructor that isn't `Send` | compile | generated impl |
| Two or zero constructors, a bad attribute, a malformed `into` item | compile | macro span error; the `into` text names the three forms (§4) |
| A controller under `providers`, as a type, behind `as dyn K`, qualified, as an `into` item or as a value | compile | autoref probe on `Controller`, a method bound + `on_unimplemented` naming the type (§4) |
| A type under `controllers`, or in `ModuleDef::controller::<C>()`, that does not implement `Handlers`: an `#[injectable]` type, or a hand-written `Controller` with no `Handlers` impl | compile | `ModuleDef::controller`'s `C: Handlers` bound + `on_unimplemented` naming the type and pointing at `#[controller]` and `#[handlers]` (§4) |
| `#[handlers]` on a type that is not a `Controller`, an `#[injectable]` type among them | compile | the `Handlers: Controller` supertrait + the same `on_unimplemented` (§4) |
| `#[controller]` on a type with no `Handlers` impl, from `#[handlers]` or written by hand | compile | the `Self: Handlers` bound on the `Controller` impl `#[controller]` writes + `on_unimplemented` naming `#[handlers]` (§4) |
| A `mount` written in a `Controller` impl | compile | `Controller` has no methods (E0407): a controller's handlers are its `Handlers::mount` (§14, item 28) |
| A marker, `#[guards]`, `#[interceptors]`, `#[error_handlers]`, `#[meta]`, `#[construct]` or a field's `#[construct(default)]`, under a qualifier other than `ulo` | compile | macro span error at the attribute, "write the marker bare, or as `ulo::<name>`" (§14, item 27) |
| A route prefix on `#[controller]` | compile | macro span error naming `.at(prefix)` (§4) |
| `#[injectable(..)]` or `#[controller(..)]` on a field of an `#[injectable]` or `#[controller]` struct; a field's `#[construct]` with no argument, with one other than `default`, or written twice | compile | macro span error at the attribute naming `#[construct(default)]` (§4, §14, item 69) |
| `X as <role key>` in a `providers` list | compile | macro span error on the role key (§4); reads the key as written, so an alias passes to the wiring error below |
| Trait binding with a non-implementing type | compile | coercion in the closure |
| `?` on a value whose expression failed | startup (`wire`) | `try_value` record, `ModuleDef`'s for a `providers` entry and `Contribute`'s for an `into` entry (§4) |
| Missing dependency | startup (`wire`) | visibility pass |
| `dyn Repo` bound as `dyn Repo + Send + Sync`, or the reverse | startup (`wire`) | missing-dependency report naming both spellings |
| Two sources for one key | startup (`wire`) | visibility pass, naming both |
| Duplicate single binding, single/collection mix | startup (`wire`) | binding pass |
| A qualified contribution under a role key the freeze recorded, `contribute::<AnyGuard<Http>>().qualified::<Q>()` | startup (`wire`) | freeze pass, roles by `TypeId` (§7); hint "contribute it unqualified" |
| A single binding under a role key the freeze recorded: `also_as`, an alias, `provide` or `value` | startup (`wire`) | freeze pass (§7); hint "contribute it" |
| Dependency cycle | startup (`wire`) | DFS with path |
| A module importing itself | startup (`wire`) | registration walk; `ImportCycle`, the module's name twice. An import cycle through other modules is accepted (§8.2) |
| An `export::<T>()` of a key the module does not bind | startup (`wire`) | module graph pass; `ExportNotBound`, with the `reexport` hint when an import provides it |
| A type registered with `ModuleDef::controller` and bound as a provider anywhere in the graph, by `provide`, a factory, a value or a contribution; or a controller registered through `provide`, `provide_with`, `try_provide_with` or `Contribute::provide` whose `Construct::dependencies` marks it | startup (`wire`) | binding pass; `ControllerProvided`, naming both registrations in the first case, reading the mark `Dependencies::mark` records in the second, which the generated `Construct::dependencies` writes by probing `Controller` (§14, item 28) |
| `ModuleDef::controller::<C>().at(prefix)` on a controller none of whose handlers' transports read a prefix, or that mounts no handler | startup (`wire`) | freeze pass against `Transport::READS_PREFIX` (`transports/DESIGN.md` §2.5), naming the controller, the prefix, the transports or the absence of handlers, and the `.at` call's location |
| Singleton → execution dependency, including through transients: an explicit singleton, `#[injectable(singleton)]`, `#[controller(singleton)]`, `with(singleton) = ..` or `.singleton(..)`, reading execution data (§3.3); for a controller declared `singleton` the help names removing `singleton`, then a factory, then `#[controller(execution)]`, and for any other type `#[injectable(execution)]` | startup (`wire`) | needs-execution pass; `ScopeViolation`, its `controller` flag choosing the hint, or `ClosureScopeViolation` for an enhancer a handler declares by closure, its transport held as a `TypeName` (§10.2) |
| An `Auto` provider reading execution data, a type or a `with` closure under a provider key alike; the hint names `#[injectable(execution)]` for a type and `with(execution)` or `.execution(..)` for a closure (§6.2) | startup (`wire`) | needs-execution pass |
| A hook on an `Auto` type inferred per-execution, a closure hook or `.ready(..)` included | startup (`wire`) | scope pass |
| A hook, readiness, module-hook or metadata closure reading `Ext`, `ExecutionRef`, an input or a per-execution key | startup (`wire`) | scope pass (§6.2) |
| A non-optional input read on a path from a transport that doesn't seed it | startup (`wire`) | per-handler input check; `InputNotSeeded`, the handler's transport and the seeder held as `TypeName`s, the steps after the handler as text (§10.2) |
| An input declared by a keyed module | startup (`wire`) | module graph pass |
| One input declared by two transports, or a transport's declaration beside a module's with another seeder or beside a binding under the key | startup (`wire`) | binding pass; `InputConflict`, each side an `InputOrigin` |
| An override that matches nothing, or more than one binding, `in_module::<M>()` over several instances of `M` included | startup (`wire`) | test builder |
| A `replace_module` whose original no module imports | startup (`wire`) | test builder; `ReplacementUnmatched` |
| A second `replace_module` of an original already replaced | startup (`wire`) | test builder; `DuplicateReplacement`, naming both calls |
| `override_value::<dyn Timer>` | startup (`wire`) | test builder; hint "set it with `TestApp::timer(..)`" (§3.9) |
| `override_value::<dyn Runtime>` | startup (`wire`) | test builder; `RuntimeOverride`, hint "set it with `TestApp::runtime(..)`" (§3.9) |
| A module binding `dyn Timer` itself | startup (`wire`) | two sources for one key, against the core's global export |
| A written wait, `After(..)`, `.timeout`, `.attempt_timeout`, `.backoff` or a builder knob, on an app with no `Timer` | startup (`wire`) | environment pass (§3.9); `BoundWithoutTimer` and `BackoffWithoutTimer` name the `.timeout(..)`, `.attempt_timeout(..)` or `.backoff(..)` call that wrote the wait, and a trait const's `After(..)` the registration that brought the type in |
| `After(Duration::ZERO)` on a construction, a readiness check or one attempt of it, or a hook, a trait const's included, with or without a `Timer` | startup (`wire`) | environment pass; `ZeroBound`, the item's `BoundItem`, its binding, its module and the call that wrote the bound, hint `.unbounded()`; a transport's own zero settings are X22's (transports DESIGN) |
| `construct_timeout(ZERO)` or `hook_timeout(ZERO)` on the builder | startup (`wire`) | environment pass; `ZeroDefault`, the knob and the call that set it, hint `.unbounded()` on the items; `drain_timeout` and `shutdown_timeout` accept zero |
| A keyed module keyed again, `store.keyed::<A>().keyed::<B>()` | startup (`wire`) | module graph pass; `NestedKeyed`, naming both qualifiers, hint to key once |
| A second import of an identity with other `Module::settings`, a store with another `connect_within` | startup (`wire`), or runtime (`load`) | module graph pass; `DifferingSettings`, naming the module, both sites and each differing setting with both values, hint to write the same settings or key one; `LoadError::Wiring` from `load` |
| A second registration of a `#[module(identity = type)]` module that has fields, imported from two modules or loaded again | startup (`wire`), or runtime (`load`) | module graph pass; `RegisteredTwice`, naming the module and both sites, hint to give it an identity (`Eq + Hash + Clone` fields, or `settings()`) to share it; `LoadError::Wiring` from `load` |
| A module's settings writing one name twice, an exporter's or an adapter's setting under a name the module appends | startup (`wire`), or runtime (`load`) | module graph pass; `DuplicateSetting`, naming the setting, the module and both `with` calls' lines, hint to write each name once |
| A binding that is not a controller, a module hook or an enhancer closure reading a controller's key: a construction, an optional read, a hook, a readiness check or an alias | startup (`wire`) | visibility pass; `ControllerRead`, naming the controller, the reader and both modules, hint "move the shared logic into a provider both can read"; a controller reading a controller, a handler parameter and a runtime `app.get::<Ctl>()` are not checked |
| A hand-written `Construct` reading a key, an extension or the execution its `dependencies` does not declare, an optional read included | startup (`connect`), or runtime for an execution-scoped or transient type | first construction; `Errored` holding a `Permanent` `UndeclaredRead` naming the key (the type, for an extension) and the reading type, in `ConnectError::Construct` or `LookupError::Construct` (§3.4) |
| A constructor, factory, readiness check or hook fails, panics or times out | startup (`connect`) | `StartupError::Connect`, the case named in `ConnectError` and the reason on `FailureReason` |
| `ModuleRef::get` for a singleton not yet built | startup (`connect`) | `LookupError::NotReady` |
| A `ModuleRef` or `ExecutionRef` read at construction that reads back an execution-scoped or transient binding still being built | runtime (`get`) | `LookupError::Reentrant` naming both keys (§8.5) |
| A WebSocket gateway on the HTTP server's port in an app that imports no `WsModule` | startup (`wire`) | `WiringError::NotImported` naming `ulo_ws::WsModule::for_root()` (transports decision 182) |
| A server's `prepare` that panics | startup (`listen`) | caught; one `ConfigureError` naming the transport, its `source` a `PreparePanicked`; the other servers still prepare and nothing binds |
| A transport bound on an app with no `Runtime`, one given `.timer(..)` alone included | startup (`listen`) | `StartupError::Bind` carrying `RuntimeMissing { transport }` in its `source`, reached by `downcast_ref`, before any server's `prepare` (§10.2); the drain and the deadlines are timed by its clock and every connection and call runs on it (§9.5) |
| A lazy module with controllers or middleware, contributions to a collection it doesn't introduce, global exports, or an execution input | runtime (`load`) | `LoadError::Refused`, the case named in `LoadRefusal` (§8.6) |
| A lazy module that fails to wire, or whose construction, readiness check or init hook fails | runtime (`load`) | `LoadError::Wiring`, `LoadError::Connect` |
| `load` from Stopping on | runtime (`load`) | `LoadError::Closed` (§9.5) |
| `execute` or `Execution::open` from Draining on | runtime | `Closed`; `open_terminal` with a `DrainToken` is allowed in Draining (§6.3) |
| A singleton lookup from Destroying on | runtime | `LookupError::Closed` |
| Lookup not found, not ready, wrong type, wrong kind, no execution, ambiguous key, ambiguous module, a build inside a call that fails or times out | runtime | `LookupError` |
| An execution input not seeded by a standalone execution, an extension not written | runtime | `LookupError::NotFound`, or `None` through `Option` |
| A guard's refusal | runtime | `GuardRejected` through the error handlers |
| A panic in a guard, interceptor, handler or error handler | runtime | caught at its stage; `PanicRecovered` through the interceptors around a handler and the error handlers (§7); `is_panic(&BoxError)` recognises it, a `LookupError::Construct { reason: Panicked(..) }` and a `ConnectError` carrying one, through wrapping `Err`s and `source()` chains (§10.2) |
| A shutdown hook panics, times out, is skipped by `shutdown_timeout` or fails to read a parameter, or a transport's `close` fails, panics or exceeds its bound, or a transport's `serve` ends, panicking included | runtime (`close`) | `ShutdownFailure` inside `ShutdownError`, `Hook` and `Close` each with a `FailureReason`, `Serve` with a `ServeEnd`; the later steps still run |

---

## 13. The integration crate surface (no macros)

This is a complete database integration written against the public API:

```rust
use fw_core::{
    scope, BoxError, Construct, ConstructError, Dep, Dependencies, Hooks, Module, ModuleDef, ModuleIdentity, OnModuleInit,
    Resolver, Secret,
};

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct DbModule { url: Secret<String>, max_conns: u32 }

impl DbModule {
    pub fn for_root(url: impl Into<Secret<String>>) -> Self {
        Self { url: url.into(), max_conns: 10 }
    }
}

impl Module for DbModule {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_value(self).label("DbModule")
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        let (url, max) = (self.url.clone(), self.max_conns);
        m.secret(&self.url);                         // redacted from every error this graph reports

        m.try_singleton(move || {                    // `connect_lazy` returns a `Result`; the `Ok` type is the key (§4)
            let url = url.clone();
            async move { PgPoolOptions::new().max_connections(max).connect_lazy(url.expose()) }
        })
        .ready(|pool: Dep<PgPool>| async move { sqlx::query("select 1").execute(&*pool).await.map(drop) })
        .retries(5)
        .timeout(Duration::from_secs(10))
        .on_destroy(|pool: Dep<PgPool>| async move { pool.close().await });

        m.provide::<Migrations>();                   // a `Construct` type: its hooks run (§9.1)

        m.contribute::<dyn HealthIndicator>()
            .singleton(|pool: Dep<PgPool>| async move { PgHealth::new(pool) }, |a| a);

        m.export::<PgPool>();
    }
}

pub struct Migrations { pool: Dep<PgPool>, set: Migrator }

impl Construct for Migrations {
    type Scope = scope::Singleton;

    fn dependencies(d: &mut Dependencies) { d.field::<Dep<PgPool>>("pool"); }

    async fn construct(r: &Resolver<'_>) -> Result<Self, ConstructError> {
        let pool = r.dep::<PgPool>().await?;                                 // a `LookupError` passes through as `Dependency`
        let set = Migrator::new(Path::new("./migrations")).await.map_err(ConstructError::failed)?;
        Ok(Self { pool, set })
    }

    fn hooks(h: &mut Hooks<Self>) { h.on_module_init(); }
}

impl OnModuleInit for Migrations {
    async fn on_module_init(&self) -> Result<(), BoxError> { Ok(self.set.run(&*self.pool).await?) }
}
```

The value API an integration writes against:

| Area | API |
|---|---|
| Modules | `Module` (with `settings`), `ModuleIdentity`, `ModuleSettings`, `ModuleDef::{import, global, export, reexport, secret, on_init, on_destroy, meta, meta_with, enhancer}`, `DynamicModule` (with `with_settings`), `Keyed` |
| Bindings | `provide::<T: Construct>`, `provide_with::<T>(factory)`/`try_provide_with`, `value`/`try_value`, `with`/`try_with` (`Auto`, §3.3), `singleton`/`try_singleton`, `execution`/`try_execution`, `transient`/`try_transient`, `contribute::<T>()`, `enhancer::<R: Role>()` (the same builder marked `Enhancer`, for a role key of a transport with no mounted handler), `Contribute<'m, U, Q, Mark = Plain>` (`provide`, `value`, `try_value(Result<Arc<U>, E>)`, `singleton`/`try_singleton`, `execution`/`try_execution`, `with`/`try_with` (`Auto`, §3.3), each with the coercion closure; `qualified` on `Plain` alone), `handle::{Plain, Enhancer}` (empty enums, the marks), `alias::<T, Q>().of::<Existing>()`, `input::<T>().seeded_by::<Tr>()` |
| Binding handles | `also_as`, `qualified::<Q>`, `timeout(..)`/`unbounded()` on the binding itself, `ready(..).retries(..).backoff(..).attempt_timeout(..).timeout(..)`/`.unbounded()`, `on_init`/`on_destroy`/`before_shutdown`/`on_shutdown` each with `.timeout(..)`/`.unbounded()` (singleton and `Auto` handles); every bound written once per item (§9.1) |
| Injection points | `FromContainer`, `Requirement`, `Dependencies::{field, param, add}`, `Key`, `KeyName`, `TypeName`, `Resolver::{dep, dep_qualified, many, many_qualified, entries, ext, input, module, execution, by_key}` |
| Construction | `Construct` (with `CONSTRUCT_TIMEOUT`), `Hooks<T>::{on_module_init, on_application_bootstrap, on_module_destroy, before_application_shutdown, on_application_shutdown}`, `hooks!`, `ConstructError` |
| Transports | `Transport`, `Role`, `Controller` and `Handlers::mount`, `Mount` (whose `handler` records the transport's role keys for the freeze, taking a `HandlerSpec` over the transport's handler value: `fw_http::HttpHandler`, `fw_rpc::RpcHandler`, `fw_ws::WsHandler` with `fw_ws::GatewayHooks`, `fw_grpc::GrpcHandler`, transports X40), `dispatch`, `Execution::{open, open_terminal, seed, handle}`, `DrainToken`, `ExecutionRef`, `EnhancerSpec` (`guard_with`/`interceptor_with`/`error_handler_with`, `Auto`; `guard_with_in::<S>` and kin take the scope, §3.3), the `Erased*` role twins |
| Runtime | `Timer`, `Spawn`, `Runtime`, `TaskHandle`, `TaskEnd`, `spawn_with`, `Bound`, `timeout`, `timeout_at`, `Expiry`, `Cut` (a reporting bound's ties, §3.9), `Signal`, `Server` (implemented by transports for `prepare`, `bind`, `bound`, the drain, which hands over the `DrainToken`, and `close`), `AppHandle`, `Cancelled`, `Draining`, `Shutdown`, `Closed` |
| Errors | `StartupError`, `RuntimeMissing`, `Refused`, `RuntimeShutDown`, `RUNTIME_SHUT_DOWN`, `ConnectError`, `FailureReason`, `Limit`, `WiringErrors`, `WiringError`, `ConfigureErrors`, `ConfigureError`, `PrepareError`, `PreparePanicked`, `LoadError`, `LoadRefusal`, `ShutdownError`, `ShutdownFailure`, `LookupError`, `UndeclaredRead`, `GuardRejected`, `PanicRecovered`, `DispatchStage`, `InputOrigin`, `BoundItem`, `ModuleSite`, `DifferingSetting`, `is_panic`, `Redacted`, `Secret` |

The transport layer extends this surface, and `transports/DESIGN.md` §11 states each extension where a requirement forces it: `Transport::KEY` and `Transport::inputs`, whose declarations an `InputConflict` reports through `InputOrigin` (§10.2); `Transport::READS_PREFIX`, by which the freeze refuses `ModuleDef::controller::<C>().at(prefix)` on a controller none of whose handlers' transports read a prefix, or that mounts none; a `HandlerSpec` builder for `Mount::handler` carrying metadata, route, shape and the handler's own dependencies, with `Execution::handler()`, `AppHandle::handlers()` and `App<Wired>::handlers()`, `HandlerInfo` carrying the request method `Transport::request_method` reads off the handler value and the module that registers the handler, and `ModuleDef::controller::<C>().at(prefix)`; `Mount::once`; impl-level shared values through `EnhancerSpec::{guard,interceptor,error_handler}_arc`; `CancelReason`, `cancel_with` on `Execution` and `ExecutionRef`, `cancel_reason` and `on_stream_end`; `Server::prepare` before any bind, `Server::bound` and `StartupError::Configure(ConfigureErrors)`, a fourth variant shaped as `Wiring(WiringErrors)` is, with `PrepareError` as what a `prepare` answers when its text names types (§10.2); `dispatch_late` and `is_late` for errors after a stream has started; `Execution::route_to`; `Mounted::module_meta` and `ModuleRef::with_execution`; `Mounted::handlers_reading` and its `InputReader` (`handler()`, `transport()`, `steps()`), built on the input walk (§6.4); `AppHandle::phase`, `AppHandle::redact` and `AppHandle::drain_timeout`; `Spawn`, `Runtime`, `TaskHandle`, `TaskEnd`, the `RuntimeTask` SPI a runtime crate implements, `spawn_with` and `ValueHandle`, with `AppBuilder::runtime`, `TestApp::runtime`, `AppHandle::timer` and `AppHandle::runtime`, and `fw-transport`'s `TaskSet` (transports §8); `RuntimeMissing`, which `listen()` and `AppHandle::mounted` answer for an app binding a transport with no runtime, and `Mounted::runtime` (transports §8, X31); `Inputs::also_seeded_by`, with the freeze merging equal seeder sets (§6.4); `AppHandle::mounted::<T>()` and `Mounted::handlers_of::<U>()`, the mounted handlers of a transport read from a handle or from another transport's server; and `TransportMetadata`, by which a metadata type names the one transport it is valid on, read by a probe the handler attributes emit. Nothing above changes for them: each is an addition at the point the transport design names.

---

## 14. Decisions taken with defaults (open to change)

1. **Collection and extension spelling.** `Dep<T>` stays the single-dependency spelling. Collections are `Many<T>` and execution views are `Ext<T>`, because they read differently and fail differently.
2. **The qualifier is a second type parameter:** `Dep<PgPool, Replica>`, defaulting to `()`.
3. **Collections are app-wide**, not limited by visibility.
4. **Module structure is synchronous.** Registration can't await. Anything that needs async configuration becomes an async factory. This keeps "no I/O before wiring completes" true.
5. **Panics in user code are caught** at poll boundaries and reported as `FailureReason::Panicked` on the error of the phase: `ConnectError` during `connect` and `load`, `ShutdownFailure` during `close`, a transport's `close` included, `LookupError::Construct` inside a call. In a call's pipeline, each stage runs under its own catch and a panic becomes `PanicRecovered { stage, message }` where it happened: a handler's passes back through the interceptors as the `Err` of `next.run()`, and every stage's reaches the error handlers, so four transports do not each write the catch. `is_panic(&BoxError)` recognises it, the `LookupError::Construct { reason: Panicked(..) }` of a container build inside a call, and a `ConnectError` carrying one, bare or inside `LoadError` or `StartupError`; it follows a constructor's or `try_` factory's wrapping `Err`, `ConstructError::Failed` and `source()` chains to 32 levels, and inside a `Redacted` looks through the core's own types alone. It takes `&BoxError`: `is_panic(&err)` on the `BoxError` a handler holds compiles against that and is E0277 against `&dyn Error`.
6. **Singletons are eager.** They're built during `connect`, never lazily on first use.
7. **Auto scope** is singleton for providers and inferred for controllers and enhancers. An explicit `singleton` opts out of inference.
8. **Collection order** is depth-first post-order over imports from the root, imports in the order written, then declaration order inside a module. Hooks and readiness checks tie-break on the same order.
9. **The transport owns the execution**, or `execute` does for a standalone one; everything else holds a clone. Cancellation fires while clones are alive: the transport fires it for a call at a disconnect or the deadline, a standalone execution's fires at its deadline, and the drain's end fires it on whatever is still alive, never on `execute` completing. The execution ends, and its instances drop, when its owner drops the `Execution`; a clone kept past that reads what the execution recorded, and its `get` answers `LookupError::ExecutionEnded`.
10. **`Many<T>` is eager.** Reading it constructs every contribution. The lazy form, `Resolver::entries::<T>()`, is a transport's surface, not an injection point.
11. **Shutdown is one event.** The first trigger wins, `serve`'s signal, a `close` on any handle, or a transport's `serve` ending while the app runs, which the outcome reports as `ShutdownFailure::Serve` (§9.5), and its signal reaches the hooks; a later `close` joins the running shutdown and ignores its own signal. Every receiver gets the one outcome, which is why `ShutdownError` is `Clone`.
12. **The drain comes before cancellation.** In-flight executions get `drain_timeout`, ten seconds unset and timed by the `Timer`, to end on their own; `draining()` is the notice at its start and cancellation the deadline signal at its end, `drain_timeout` or the earlier expiry of `shutdown_timeout`. Those still alive are abandoned and counted in `Shutdown::abandoned`. Without a `Timer` the drain is zero-length.
13. **`BeforeApplicationShutdown` runs while the app still serves.** The order is before-shutdown, stop accepting, drain, destroy, sockets, shutdown. `load` is refused from Stopping on with `LoadError::Closed` and `execute` from Draining on with `Closed`; a singleton lookup from the first destroy hook on is `LookupError::Closed`.
14. **Every bound is a `Bound`.** A hook defaults to `hook_timeout` (10 s); a construction, and one attempt of a readiness check, to `construct_timeout` (30 s). Every bounded item is in one of the same three states, default, explicit or unbounded, written where it is declared and written once: after `.timeout(..)` or `.unbounded()` the handle's state has neither method, and a readiness check writes `.timeout` and `.attempt_timeout` once each or `.unbounded()` in place of both. The defaults apply only with a `Timer`, an explicit bound without one is a wiring error, and `Unbounded` needs none. `shutdown_timeout` caps the whole sequence from the trigger and is unset by default.
15. **A timed-out hook is dropped and reports which limit fired, the item's own, one attempt's, the app default or the cap, with that limit's duration; a timed-out execution is cancelled and left running.** The first stops at its next await, the second is still held by its transport or subtask.
16. **Disconnect handlers at shutdown are best effort.** Terminal executions run inside the drain window and no grace window follows it; `Shutdown::terminal_skipped` counts the losses, and state that must survive a crash lives in TTL or heartbeat storage.
17. **Redaction is scoped by origin and stored as a type.** Every error the core did not create itself, from user code, integrations or transports, passes through the redaction function before it is stored in any core error type; the scope is the origin, with no list of variants to maintain. What is stored is a `Redacted`: `Display` and `Debug` write the text, `source()` is `None`, and the original is reached through `downcast_ref` and `original` alone, which is what lets an error handler map a constructor's domain error to a status.
18. **A role is decided by `TypeId` at freeze.** Mounting a handler records its transport's three role keys, and every contribution under one of them is an enhancer however it was registered, so an alias or a `macro_rules` wrapper changes nothing and no type name is read. `m.enhancer::<R: Role>()` marks a contribution explicitly for a role key of a transport with no mounted handler, bounded by the sealed `Role` trait the three role keys implement, and returns the `contribute` builder marked `Enhancer`, on which `qualified` does not exist; one marked contribution marks the whole key. The freeze refuses two forms under a recorded role key, each of which nothing reads: a qualified contribution, with the hint "contribute it unqualified", and a single binding, through `also_as`, an alias, `provide` or `value`, with the hint "contribute it". A role key no mounted handler reads and no `enhancer` marks is not checked. Every `into` list in `#[module]` lowers to `contribute` and takes a type, `value = expr`, `value = expr?` or `with = closure`, the closure `Auto` unless it writes `with(scope)` (item 22). No `#[module]` spelling marks a role key of a transport with no mounted handler; that is a hand-written `m.enhancer::<K>()`, anywhere in the graph.
19. **Diagnostics print short type names, and full paths on collision.** Every type name in a report is a `TypeName`, cut to its last path segment, inside generic arguments too. Where one report would print two different types alike, a key's type or qualifier or a transport's marker, those print with their full paths; module types follow the same rule under their own check. An entry stores the names it reports as values and renders them when the report is formatted. Descriptive text may be stored rendered, its names short, and never carries a name the entry also stores as a value (§10.1).
20. **A transport's `close` is bounded, and the closes run concurrently.** Each runs under what is left of `shutdown_timeout`, or under `hook_timeout` when no cap is set, so two servers each waiting out a grace for their connections cost one grace, not the sum; the failures are reported in bind order, not in the order the closes ended, so the report is deterministic. One that exceeds its bound is dropped and recorded as `ShutdownFailure::Close { transport, reason }`, the shape `Hook` has, `reason` a `TimedOut` naming `ShutdownCap` or `Default`, an `Errored` for the transport's error and a `Panicked` for a panic, caught as a hook's is; a `close` can wait on a TLS close-notify or a broker's acknowledgment, which is what the cap exists to contain.
21. **A written wait is refused where `wire()` can see it.** `.backoff(..)` on an app with no `Timer` is a wiring error like any written bound. A standalone deadline on such an app is stored and never fires: it is a runtime value passed to `execute`, whose only error is `Closed`, and refusing it would widen that error for one rare case on an app `listen()` already confines to standalone use.
22. **Scope is its own axis.** `with = closure` says how a binding is built and nothing about how long it lives; its scope is `Auto`, with the meaning item 7 gives, and the same closure means the same thing on a method, on a controller impl, in an `into` list, in a `providers` list, where a bare closure is shorthand for it, and through the value API. One override serves every position: `with(singleton | execution | transient) = closure`, the three words `#[injectable(..)]` takes; `ModuleDef::singleton`/`execution`/`transient` and `Contribute::singleton`/`execution`/`transient` beside each one's `with`/`try_with`; `EnhancerSpec::guard_with_in::<S>` and kin beside `guard_with`. An explicit `singleton` that needs an execution is refused at `wire()` as a type's is. Inference reads what a closure reads, not what it does: a closure that creates per-call state and reads nothing writes `with(execution)`. `value = expr` is built once and shared wherever it is declared.
23. **`.runtime(..)` and `.timer(..)` compose.** `.runtime(r)` sets the spawning and the clock, `.timer(t)` the clock alone, each overriding only what it sets, in order (§9.4). A later `.timer(t)` that cleared the runtime would leave `.runtime(r).timer(t)` unable to spawn, failing far from the line that caused it, and that pairing is what a test wants, a real executor under a fake clock. After both, `dyn Runtime` is a composite timing with `t`, so the app has one clock whatever reads it; the cost is that `Dep<dyn Timer>` and `Dep<dyn Runtime>` are then two objects that agree, where `.runtime(r)` alone binds one.
24. **A task's panic keeps its message, and a panic while a task's future is dropped is logged at `warn`.** `TaskEnd::Panicked(Arc<Redacted>)` is converted by the one `redact_panic` the core uses for `PanicRecovered` and `FailureReason::Panicked`, with the app's registry for a task spawned through the app's runtime, whose wrapper carries it, and an empty one for a task spawned on a bare runtime outside any app (25; §3.9, transports §8). The drop-time panic reaches no caller, so the wrapper catches it where the drop runs, keeps the end `Aborted`, and logs it through `tracing`, the core's one log line and its dependency for it; uncaught, tokio swallowed it and smol aborted the process.
25. **The app's runtime is always the core's wrapper, carrying the graph's redactor.** `.runtime(r)` binds `dyn Runtime` and `dyn Timer` as one `Clocked` around `r` in every configuration, `.runtime(r)` alone included, so a task spawned through the app, `Dep<dyn Runtime>` or `AppHandle::runtime()`, has every registered secret replaced in its panic's message and in its drop-time `warn`; binding `r` itself left nothing of the app between a caller and `r`'s `spawn`, and a panic message is where a connection string ends up. The redactor is set from the graph at `wire` and refreshed by every `load`, the base graph's put back on a failed connect, and read at the panic rather than at the spawn, so a secret a lazily loaded module registers is replaced in a task spawned before the load that panics after it. The cost is a second `CatchUnwind` and record per task spawned through the app, and that the object injected is the wrapper rather than the object given, so a reader wanting `r`'s own type cannot reach it through `Dep<dyn Runtime>` or `AppHandle::runtime()`, a downcast included, which both accessors' documentation says in a sentence, there being no `Any` supertrait on `Runtime` for a downcast to try; a task spawned on a runtime outside any app keeps the userinfo strip alone (§3.9, §9.4, transports §8).
26. **`unsafe_code` is `deny` with listed opt-outs, not `forbid`, and a script holds the list** (§0, transports §0, transports 113). `forbid` reaches each member as `-F unsafe_code`, under which a module's `#![allow]` is itself an error, so it admits no per-module opt-out; `deny` can be relaxed anywhere, which is what `scripts/unsafe_allowed.py check` refuses outside `ci/unsafe-allowed.txt`. The list holds two modules, each marked by kind: `fw-server-hyper`'s `FuturesIo`, pure Rust, whose tests the `miri` job runs under Miri, failing when it runs none of a pure-Rust entry's tests (F388); and `fw-net`'s descriptor adoption, a system call Miri cannot execute, printed as skipped with the reason and resting on its safety comments and ordinary tests (transports 113, 116).
27. **Which attributes are imported, which name their transport, which are written bare.** The core attributes (`#[controller]`, `#[handlers]`, `#[injectable]`, `#[module]`) are imported. A transport's attributes name their transport, `#[ulo_http::get]`, `#[ulo_rpc::message]`, `#[ulo_ws::message]`, `#[ulo_grpc::method]`, `#[ulo_ws::gateway]`, because the path says which transport the attribute belongs to and `message` exists in both `ulo_rpc` and `ulo_ws`; they are not renamed to be distinct, the full path is the distinction. Markers that mean the same on every transport, `#[guards]`, `#[interceptors]`, `#[error_handlers]`, `#[meta]`, `#[construct]` and the field marker `#[construct(default)]`, are written bare, read and removed by the macro around them before Rust resolves them, as `#[serde(..)]` is. The one qualifier a marker takes is `ulo`, `#[ulo::guards]` or `#[::ulo::guards]`; any other, a typo, a path two segments deep or a renamed dependency's name, is a compile error at the attribute, "write the marker bare, or as `ulo::<name>`" (F595). A macro cannot resolve a path, and a crate depending on `ulo` under another name aliases it to `ulo` for any expansion to compile (below), so `ulo` is the one qualifier that means something. The core attributes and the transport attributes are a separate family: each is a real attribute macro, Rust resolves its path, and no macro reads that path. A typo in one is rustc's own unresolved-path error, and any path that reaches one is accepted, `#[crate::controller]` beside `use ulo::controller;`, a module or a facade crate re-exporting it, `#[framework::controller]` under a renamed dependency, or another crate's macro expanding to it. The markers are not real macros that stay in the output, because Rust allows no attribute macro on a struct field or a function parameter, so field and parameter markers can only be inert, and making only the method- and impl-level ones real would split the markers among themselves and add an import to every file that writes one. For the editor, `#[handlers]`, `#[injectable]` and `#[controller]` emit beside the item a hidden `use ::ulo::<name> as _` per marker, spanned at the user's own token, so hover and go-to-definition reach the macro's documentation; a marker written `#[ulo::guards]` gets a `use` built from both of its written tokens, so its `ulo` segment answers for the crate as well. Every path the macros generate names `::ulo`, so a crate that depends on it under another name writes `extern crate framework as ulo;` at its root for any macro to expand, and the hidden import resolves wherever the rest of the expansion does.
28. **A controller is declared with `#[controller]`, and its handler impl with `#[handlers]`, on every transport.** `#[injectable]` says two things: the container builds the type, and other bindings inject it. A controller is built by the container and reached through its transport, and `wire()` refuses a read of one by any binding that is not itself a controller (`ControllerRead`): outside the controllers, it has the first property without the second, and it takes an attribute of its own. What makes a type a controller is the public trait `Controller: Construct`, a mark with no methods, and every check reads that trait. A controller's handlers are declared by `Handlers: Controller`, whose `mount` is the one place a controller mounts anything: a `Controller` impl has no `mount` to write, so no controller can mount something in place of its handlers, or nothing at all while its handlers go unmounted. A controller written by hand implements `Construct`, `Controller` and `Handlers` itself, and marks its type in `dependencies` with `d.mark::<Self>()`. `#[controller]` writes the `Construct` impl `#[injectable]` writes, from a struct or from an impl's `new` or `#[construct]` fn, async and fallible alike, takes the same scope words and `timeout`, and writes beside it `impl Controller for Self where Self: Handlers {}`. The bound is what makes a `#[controller]` type with no `Handlers` impl a compile error at the attribute: on a concrete type it is false where it is written, whether or not a module registers the type. A generic controller compiles with or without it, since registration asks for `Handlers`, and is a controller for each type its `#[handlers]` impl covers. `#[handlers]` writes `impl Handlers`, and `Handlers: Controller`, so `#[handlers]` on a type that is not a controller, an `#[injectable]` type among them, is a compile error naming `Controller` and pointing at `#[controller]`: a handler impl no module could mount is refused where it is written. A `#[controller]` type with no `Handlers` impl is a compile error naming `#[handlers]`; one that mounts its handlers by hand, a WebSocket gateway implementing `Gateway` among them, implements `Handlers` itself. `#[module]`'s `controllers` lowers to `ModuleDef::controller::<C>()`, whose `C: Handlers` bound takes a controller written either way and refuses any other type at the entry, a hand-written `Controller` with no `Handlers` impl among them. rustc reports the unmet `Handlers` bound alone for a type that is no `Controller` either, so `Handlers`'s `on_unimplemented` names both attributes. Under `providers`, `#[module]` refuses a `Controller` at every entry whose type it can name, a type entry or a value (a closure is not checked), by an autoref probe on the trait (`__private::controller_entry`). `ModuleDef::provide` and its `_with` forms cannot refuse a trait at compile time without negative bounds, and a generic `T: Construct` cannot ask whether `T: Controller`: the only thing `provide` reads of `T` is its `Construct` impl. The `Construct::dependencies` that `#[injectable]` and `#[controller]` write therefore ends with the same probe on `Self`, run where the type is concrete, which calls `Dependencies::mark::<Self>()` when `Self: Controller`; `provide`, `provide_with`, `try_provide_with` and `Contribute::provide` read the mark, and `wire()` refuses a type registered through them that the mark names (`ControllerProvided`). A registration reads the mark of its own type only, so a hand-written `dependencies` calling a controller's declares its reads without marking its own type. The mark is the compiler's answer for the trait, and holds for an `#[injectable]` type whose `Controller` impl is written by hand as for a `#[controller]` type. A `Construct` impl written by hand marks its type by calling `d.mark::<Self>()`, public for that use, whose `C: Controller` bound refuses to mark a type that is not a controller. Apart from any mark, `wire()` refuses a type registered with `ModuleDef::controller` in any module and bound as a provider in any module, by `provide`, a factory, a value or a contribution, naming both registrations (`ControllerProvided` with `controller_at`): a binding records the `TypeId` its recipe builds, compared with the controllers'. That reaches a hand-written `Construct` that does not mark, `m.value(C)`, `m.with(..)`, `m.singleton(..)` and `Contribute::value` whenever the type is also registered as a controller, which is the shape the mistake takes, one entry per provider registration whichever check finds it. An alias builds nothing of its own and is not a provider registration. What neither check sees: a type the graph never registers as a controller, bound by value, by a factory closure, or through a `Construct` impl that does not mark it; a contribution by value of an `Arc<dyn K>`, whose record knows only the collection's type; and a generic type whose `Controller` impl covers some instantiations only, which the probe in the generic `dependencies` reads as unmarked, unless that instantiation is also registered as a controller. Seeing these at compile time takes negative bounds or specialization, and at `wire()` the record holds nothing that names them. A controller may be exported, for another controller in an importing module to read. The list a controller sits in, the `ControllerRead` check and the `Auto` inference (§3.3) then all follow the one declaration, and `ScopeViolation`'s help for a controller declared `singleton` reads "remove `singleton` (an `Auto` controller is built per call when it needs to be), or inject a factory.", with `#[controller(execution)]` as a third option on the line after: `Auto` is what builds a controller that needs request data per call without saying so. Its field marker is `#[construct(default)]`, the one `#[injectable]` reads (item 69). It takes no route prefix: the prefix belongs to the module that mounts the controller, `m.controller::<T>().at(prefix)` (transports §2.5). A WebSocket gateway is a controller and takes it too. The handler impl is `#[handlers]` because handler is the transport-neutral word the core uses (`HandlerInfo`, `HttpHandler`, `RpcHandler`, `Execution::handler()`) and one impl attribute serves HTTP, RPC, WebSocket and gRPC handlers alike; HTTP keeps `route` where it means routing (`HttpCx::route()`, route patterns). `#[controller]` and `#[handlers]` stack on one impl in either order, and a constructor-style controller writes `new` beside its handlers. `#[construct]` does not make a method a handler, which lets the fn it marks sit in a stacked impl whichever attribute expands first.

29. **Module import cycles are accepted, and a module importing itself is refused.** An import is a visibility fact: it decides which exports a module sees, not what is built first. The cycles that break construction are dependency cycles, refused at §10.1 step 4 with their path. Nothing reads the import graph as acyclic. Collection order is a depth-first post-order that does not follow an import closing a cycle, so every module is placed once (§3.2). The connect walk, the hooks and the readiness checks order by binding dependencies, collection order the tie-break (§9.2). A visibility table reads a module's direct imports and the globals (§8.2), and re-exports resolve round by round until a round resolves nothing, which ends on a cycle as on a tree. A lazy load's modules are imported by no frozen module, so a cycle never crosses into a load (§8.6). A `ModuleRef` reads its module's table, not the imports. A module importing itself is refused as `ImportCycle`, the one entry that import produces (§8.3): a module sees its own bindings without the import. A module inside a cycle reads `ModuleDef::qualifier` by the rule a module two importers share follows, the key of the path the walk reached it by first (integrations §4.3, decision 69): the members of a cycle are each other's importers. A cycle adds no case the diamond lacks: the path the walk reaches a member by is a chain of written imports, so which member it enters first decides the key as the order of two importers decides it in a diamond. The rule is deterministic in declaration order, the first declared import path deciding. It is not independent of that order: `ModuleDef::qualifier` is read at registration, before the module's imports exist, and a `DynamicModule`'s closure runs once, so an order-independent answer would need a handle the store module reads after freeze, which nothing provides. An inherited key is what `ModuleDef::qualifier` answers and changes no key: a keyed module's exports are requalified by its own key alone (§8.3).

30. **The process's panic hook is the application's, and the core offers a redacting one it never installs** (§10.2, 17, 24, 25, Q29-17). The hook Rust runs for every panic prints the message before any `catch_unwind` the core holds sees it, and the default hook prints it raw, so the redaction of a caught panic never reaches what is already on stderr; `ulo::secrets`'s module docs say so, once, for every crate. `ulo::panic::install_redacting_hook()`, called in `main`, replaces the hook with one printing `panicked at <location>:` and the message through `scrub`, the function `redact_panic` applies, then the backtrace or the default hook's `RUST_BACKTRACE` note, reading `RUST_BACKTRACE` as the default hook reads it and not `RUST_LIB_BACKTRACE`, which `Backtrace::capture` would read first. The hook runs inside no app, so it reads a process-wide registry: `Redactor::set` records each graph's secrets into it when an app takes the graph at `wire` and at each `load`, and `collect_secrets` records what it collected; it is never emptied, and no graph's own redaction reads it, each graph keeping its registry. Before anything is recorded the hook has the userinfo strip alone. It does not chain to the hook it replaces: a hook reads the message from a `PanicHookInfo` nothing outside `std` can build, so the previous hook would print the raw message, and an application with a hook of its own keeps it and forgoes this one. A panic inside a panic hook aborts the process, so the hook unwraps nothing, ignores a failed write, leaves out the thread's name, whose read panics after the thread's local data is gone, and reads the registry with `try_read` alone, a bounded number of times, printing the message withheld when a writer holds it throughout; a writer holds the lock for a pointer swap, which cannot panic, so a thread panicking inside it is not a case the hook meets. Tests: `panic.rs`'s unit tests, the report redacting a registered secret and a url's userinfo, and the lock held on the reading thread answered withheld, which with a blocking `read` hangs; `tests/panic_hook.rs`, a child process per scenario printing a panic carrying a registered secret and a password, raw with the default hook, redacted with this one whether it is installed before the wire or after and once the app has dropped, and with the userinfo strip alone before any app wired, and the backtrace printed under `RUST_BACKTRACE=1` with `RUST_LIB_BACKTRACE=0` and not under `RUST_LIB_BACKTRACE=1` alone. With the report printing the raw message, three child tests and the report's unit test fail; with `Redactor::set` recording nothing, two.

31. **A failed `connect` or `load` tears down what had initialized** (§9.2, §9.5, Q32-4). The `OnModuleDestroy` hooks of every binding and module that had initialized run in the exact reverse of their startup order, each under its bound and the whole under a `shutdown_timeout` cap started as the teardown begins, and what they record is reported beside the walk's failure (item 66). A drop releases pools and handles, and not what an init hook did outside the process: a registration with service discovery, a distributed lock, a lease; a failed start that left the service registered as healthy would be the cost. Close is the model, with three differences that follow from an app that never served: no before-shutdown or shutdown hook runs, there is no drain, and the cap starts at the teardown rather than at a trigger. A binding with no init hook counts as initialized once it is built and checked (item 60), and an owner with no init hook waits for the singletons its destroy hooks read (item 65).

32. **A failed per-execution build is kept for the execution** (§3.8, Q32-5). §3.8's promise is one build per key per execution, and running a failing construction once per reader repeats its side effects in one execution, where a retry rarely succeeds. Concurrent readers share the attempt and later readers get the kept error; each reader's error holds the one original behind a shared pointer, so `downcast_ref` maps it for every reader alike and `original` hands every reader the same error. The app's cache of enhancer closures built once keeps no failure, since it would outlive every execution.

33. **A read back into a construction under way is refused** (§8.5, Q32-3). A `ModuleRef` or `ExecutionRef` read at construction carries the construction and those whose reads led to it, and a `get` through it that reaches an execution-scoped or transient binding still being built answers `LookupError::Reentrant` naming that binding and the reader, where the first would wait on its own build and the second recurse until the stack ran out. It is the shape the `NotReady` a singleton gets during `connect` has. Refusing every handle read during a construction would be stricter and would rule out the one pattern the handles exist for, a handle a construction keeps for later: a kept handle carries a construction that has ended, and refuses nothing. An execution opened through a handle, by `ModuleRef::execute` or `Execution::open`, carries the handle's construction, since a cycle through one opens a new execution per step and its cache refuses nothing: without the chain the reads would recurse until the stack ran out, which is not an acceptable failure for a wiring mistake (item 61).

34. **An `into` item naming a type its own module provides is that binding** (§4, Q32-11). `providers = [Alpha, into dyn Plugin: [Alpha]]` builds one `Alpha` and runs its hooks once, the collection's item the object a direct read returns, which is what a reader of the list expects; refusing the form would push users to a wordier spelling for the obvious case. The macro lowers the item to the value API's sharing form, a contribution whose factory reads `Dep<Alpha>` and widens the `Arc` it holds, declared in `Alpha`'s own scope: an execution-scoped `Alpha` is one instance per execution, the item and a direct read alike, and a transient one is built per read, as a direct read builds it. A type the module does not provide is contributed as a binding of its own, built and hooked as one. The types are matched as written, the macro seeing no types: `Alpha` and `crate::Alpha` are two spellings, and the second is its own binding.

40. **`ErrorHandler::handle` takes the error first, then the context** (§3.7, Q32-6). The trait is named for its role, as `Guard` and `Interceptor` are, and its method is `handle`: it handles an error. The order differs from `Guard::can_activate(cx)` and `Interceptor::intercept(cx, next)` because the error is the handler's primary input and the context is what it reads to shape the reply; NestJS orders `catch(exception, host)` alike. The rebuild has no `#[catch]` attribute, so naming the method `catch` would match nothing a user writes.

50. **An override drops the replaced binding's readiness check and hooks, and keeps what it read** (§11, Q31-11, F649). A `.ready(..)` check and a hook are written against the implementation a recipe builds: a mail client's check dials its host, its destroy hook closes its pool. Kept on an override, the check still runs and fails the test's `connect` on a host the fake never needed, so a test would have to replace the whole module to get past one binding. Every hook goes, not only the check and the destroy hook: an init hook and a destroy hook are commonly a pair, and the type's own trait hooks are its implementation's too, so a same-type value given to `override_value::<T>` runs none of them. The bindings the replaced recipe read are left as they are, built in connect order, because a binding is not owned by one reader and another may read it; a test whose fake must not reach them uses `replace_module`, which removes the module's whole subgraph. Overriding an `also_as` key splits it off and leaves the original binding, still built for its own key, its check and hooks. Tests: `tests/testing.rs`, a failing check and every closure hook dropped with the configuration the recipe read still built, the check failing `connect` without the override, a `Construct` type's trait hooks dropped from a value of that type, an `also_as` override leaving the original's init hook, and `replace_module` leaving a module only the original imports out of the graph.

60. **A binding with no init hook has initialized once it is built and checked** (§9.2, item 31, Q34-9). What a startup teardown runs then does not depend on where a destroy-only binding sits relative to the failure: a pool bound with only a destroy hook is closed when an init hook fails after it or before it, and when a later construction or readiness check fails, as close would close it. Position would make the outcome hang on the tie-break that orders two unrelated bindings. The binding whose readiness check failed has not initialized and is not torn down. A module with no init hook follows its singletons (item 64). Reverse startup order holds over what is torn down. Tests: `tests/startup_teardown.rs`, a destroy-only binding on either side of a failing init hook, the destroy-only bindings before a construction failure and a readiness failure, and a failed `load`'s.

61. **An execution opened through a `ModuleRef` carries its construction chain** (§8.5, item 33, Q34-10). `ModuleRef::execute` and `Execution::open` give the new execution the handle's chain, and every resolver and handle the execution gives starts from it. A cycle through `execute` opens a new execution per step, whose cache holds no build under way, so without the chain an execution-scoped binding would build again in each and a transient one recurse until the stack ran out. A handle kept past its construction carries an ended chain, and its executions refuse nothing. `App::execute` and `AppHandle::execute` start from no construction, since neither handle is an injection point. Tests: `tests/module_ref.rs`, a transient and an execution-scoped cycle through `execute` and one through `Execution::open`, each on a 128 MiB-stack thread under a depth counter, and a kept handle's execution building the binding again.

62. **A setting URL's kept text that could precede an `@` of the original is `[unreadable]`** (§8.3, Q34-19). Readers disagree on where an authority ends: RFC 3986 ends it at `/`, `?` or `#`, and a reader taking the authority to the last `@` reads `u:12?34` in `amqp://u:12?34@h/v` as the userinfo. Reporting what precedes the cut, `amqp://u:12`, would print part of a password, so one rule covers every shape: an `@` anywhere after the start of the host kept leaves the URL `[unreadable]`, which also reports a query or fragment holding an `@`, `http://host?login=me@corp`, as unreadable. A list a client splits at its commas, NATS servers and Kafka brokers, goes through `setting_url_list`. A comma may sit in a password or a query value, so the text after it may continue the element before it, until an element opens with a scheme or `//` and so starts another URL: inside one URL an element is `[unreadable]` when a later element holds an `@` or an earlier one a `?` or `#`. A password holding an unencoded `@` and a comma, `u:p@a:1,2@b`, then leaves `a:1` out, and a list whose every server opens with its scheme reads in full, `nats://u:p@a:4222,nats://u:p@b:4222` as `nats://a:4222,nats://b:4222`; a credentialed list written without schemes reads only its last element. A credential that itself holds `<scheme>://` or `//` where an element starts still opens a URL, the text before it read as a URL of its own (`u:12,//x@h:1` as `u:12,//h:1`) and its scheme reported as written, the one shape the rule does not cover: a generated run of 100,000 URLs and 100,000 lists drawing credentials from `:`, `@`, `,`, `?`, `#` and `/` among other characters left 29 reports holding part of one, every one of this shape (`redact.rs`'s `no_generated_credential_reaches_the_report_outside_a_scheme` runs a fixed seed of it). Tests: `redact.rs`'s unit tests, the cut shapes and the list rule, and each broker link's `endpoint` test.

63. **A `#[cfg]` on a constructor parameter or a constructed field gates everything the macro writes for it** (§5, Q34-36, F672). `#[injectable]` and `#[controller]` receive the `cfg` unevaluated, as `#[handlers]` does a handler parameter's (transports §2.1), and copy it, read through `cfg_attr`, onto the dependency's declaration, its read and its place in the call or the struct literal, a defaulted field included, so a parameter or field compiled out is neither declared, read nor passed. A tuple struct whose gated field would renumber the fields after it is built positionally, each value under its field's gates; a tuple struct with no gated field keeps the named form, `Self { 0: .. }`. A wiring report names a gated tuple field by its index as written, not as rustc renumbers it. A configured `#[module]` gates each field's secret probe the same way, and refuses a gated tuple field that has a field after it, since the probe reads it by position. Tests: `ulo-codegen-tests`'s `tests/cfg_construct.rs`, both forms of both attributes with a gate off, a gate on and one through `cfg_attr`, and a gated-on dependency left unbound reported by `wire()`; `ulo-macro-lints`'s `di.rs`; `tests/module_secrets.rs`, a gated module field beside a gated-on secret, and the `module_tuple_gated_field` refusal.

64. **A module with no init hook has initialized once every singleton it registers has** (§9.2, item 60, Q35-8). A module is not built, so its own hooks have nothing of theirs to wait for but its providers: a destroy hook on a module with no init hook closes what those providers opened, and it runs when they ran. The rule gives a module the position independence item 60 gives a binding: an init hook failing before the module, after it or a construction failing later tears it down when its singletons all initialized, and leaves it when one of them did not, its own destroy hook running before theirs in reverse startup order. A module registering no singleton has initialized once the singletons its destroy hooks read have (item 65). A module with an init hook has initialized once that hook completes. Tests: `tests/startup_teardown.rs`, a destroy-only module after a failing init hook with destroy-only singletons and with a singleton whose init hook never ran, one before a later module's failing init, one before a later construction failure beside a module registering no singleton, and a module after the failure whose own init hook never ran.

65. **An owner with no init hook also waits for the singletons its destroy hooks read** (§9.2, items 60 and 64, Q36-1, F676). Initialized means that everything the destroy hook needs exists. A closure destroy hook reads its parameters when it runs, and a teardown after a construction or readiness failure would otherwise run one whose parameter was never built: the read fails, the body never runs, and the teardown reports a failure for a hook that could not have run beside the failure that explains it. The reads are the hook closures' declared dependencies, resolved in the owner's module, a transient followed through what it builds; a trait hook reads only its own instance. A value with no readiness check exists from `wire()`, a lookup answering it before the walk reaches it, so a hook reading one waits for nothing; a singleton whose readiness check failed is built and not checked, and a hook reading it waits, as that singleton's own destroy hook does not run. They are not edges of the connect order, which the destroy hooks do not change: the rule decides which owners count as initialized when the walk fails, and a singleton outside the walk, one of the graph a `load` extends, has already connected. An owner with an init hook needs no such wait, the init stage starting once every singleton is built and checked. A destroy hook reading a singleton that can never be built leaves that singleton's construction failure as the one report, which is how the misconfiguration is caught. Tests: `tests/startup_teardown.rs`, a module registering no singleton and a destroy-only binding whose hooks read a singleton whose construction fails, one reading it through a transient, a module whose hook reads a singleton checked before the failure, a destroy-only binding and a module whose hooks read a value declared after the failure, a module whose hook reads a singleton whose readiness check failed, and a failed `load` whose module's hook reads the base graph.

66. **A connect failure's teardown is a field beside it** (§9.2, §10.2, item 31, Q36-2). `StartupError::Connect { error, teardown }` and `LoadError::Connect { error, teardown }` hold the walk's own failure in `error` and what its teardown recorded in `teardown`, empty when it recorded nothing. The original failure is what a caller looks for, and a field keeps it in one place whatever the teardown did: `Connect { error, .. }` followed by a match of `*error` against `ConnectError::Hook { .. }` holds whether or not a destroy hook failed. `error` is boxed (item 67). Both variants are `#[non_exhaustive]`, as every public error type is (§10.2): code outside the core reads them and builds none, and a pattern writes `..` beside the fields it names, so a later field beside `teardown` changes no `match`. `Display` writes the failure, then a line counting the teardown's failures and one per failure. `is_permanent` and `is_panic` read `error` alone: whether a start can succeed, and whether it failed by a panic, are the walk's failure's to answer. Tests: `tests/startup_teardown.rs`, a panicking destroy hook beside an init hook's failure, read through `teardown` and the text, and `is_panic` false for it; a destroy hook whose read fails on a `Permanent`, and `is_permanent` false for the start; a failed `load`'s panicking destroy hook beside its construction failure.

67. **Every public error type stays under 128 bytes, and a `const` assertion holds it there** (§10.2, Q37-12, Q37-13, Q37-15, F513, F660, F681). Clippy's `result_large_err` fires on an `Err` of 128 bytes or more, in the crate that names the `Result`: an `allow` in the framework silences its own functions and leaves every user's `fn start() -> Result<(), StartupError>` tripping the lint, so the framework's functions carry none. What is boxed is the payload that would cross the line, inside its variant, never the whole error: the `ConnectError` in `StartupError::Connect` and `LoadError::Connect`, which leaves them at 48 and 112 bytes where they are 184 inline; the `FailureReason` in `LookupError::Construct` and `ShutdownFailure::Hook`, 96 and 88 bytes; `ulo_ws::ConnectRefused`'s private headers, 40 bytes where it is 128 with them inline; and a `Redacted`'s two private fields in one allocation, 8 bytes where it is 40, which shrinks every error holding one with no public type changed. `ConnectError` itself is 136 bytes and never an `Err` unboxed, the core's own functions returning it boxed too. Boxing `module` and `reason` inside `ConnectError` instead leaves `StartupError` eight bytes under the line, which the next field would cross; it was the alternative. Stable Rust has no box patterns, so a nested pattern is written as two: `Connect { error, .. }`, then a match of `*error`. Each pin is `const _: () = assert!(size_of::<T>() < 128, ..)` beside the type, so a field taking a type over the line fails the build where the field is added. `Redacted` hands out its original through `downcast_ref` and `original(&self) -> &(dyn Error + 'static)`, both borrowing, a shared original included; an owning `into_inner` was the alternative, and for a shared original it could answer only a box of the core's own type, which `downcast` cannot read. The struct variants `StartupError::Bind` and `ConnectError::{Construct, Readiness, Hook}` carry `#[non_exhaustive]`, as `StartupError::Connect` and `LoadError::Connect` do (item 66). Tests: the pins themselves, a build failure when a field crosses the line; `tests/redacted.rs`, `original` reaching a constructor's error and its `source()` chain for a fresh and for a shared failure; `ulo-codegen-tests`' `ui/error_variant_without_rest.rs`, a pattern on `ConnectError::Hook` and on `StartupError::Bind` without `..` refused.

68. **A metadata type needs no `Default`** (§13, Q37-16). `Meta` is `Send + Sync + 'static`: a type such as a module's owning team has no sensible empty value, and a `Default` written to satisfy the bound would be a value no module meant. The module's first write builds the value, `m.meta::<T>()` from `Default` for a type that has one and `m.meta_with(init)` from `init` for any type, and every later write configures it in place; `meta` lowers to `meta_with(T::default)` (§0, item 8). A reader, `ModuleRef::meta` and `Mounted::module_meta`, answers `None` or nothing for a module that wrote none, and builds no default. Tests: `tests/module_ref.rs`, a metadata type with no `Default` built by its first write and configured by the second.

69. **A field set from `Default` is marked `#[construct(default)]`, on an `#[injectable]` and a `#[controller]` struct alike** (§4, item 28, Q38-15). The marker says what the container does with the field: it constructs it from `Default` instead of injecting it. A marker named after the container attribute, `#[injectable(default)]`, repeats that attribute on the one field it does not inject, and splits into a second spelling, `#[controller(default)]`, under the other attribute; one spelling serves both. `construct` is also the constructor's mark, so the hidden import the macro writes for a removed marker is `use ::ulo::construct as _` for either use, and an editor's hover on the field marker shows the `construct` attribute's documentation, which describes both. A bare `#[default]` is the helper attribute of `#[derive(Default)]`, which an editor resolves to std's. The impl form reads `#[construct]` on a fn and the struct form reads it on a field, and each removes what it reads before rustc sees the item, so neither form meets the other's use. The marker takes the one argument `default`; a field set from a computed value is written in the constructor style, and `#[construct(with = expr)]` is the spelling left for a field marker taking a value. Refused at the attribute, each naming `#[construct(default)]`: `#[injectable(..)]` or `#[controller(..)]` on a field of either container, "a field set from `Default` is marked `#[construct(default)]`, not `#[injectable(default)]`"; a field's `#[construct]` with no argument or another one, "a field takes `#[construct(default)]` alone"; the marker written twice on one field. On a field of a struct that carries neither attribute nothing reads the marker, and rustc refuses it as an attribute macro on a field, or as an unknown attribute where `construct` is not imported. Tests: `ulo-codegen-tests`' `ui/injectable_marker_on_an_injectable_field.rs`, `ui/controller_marker_on_a_controller_field.rs`, `ui/injectable_marker_on_a_controller_field.rs`, `ui/controller_marker_on_an_injectable_field.rs`, `ui/bad_field_attribute.rs`, `ui/construct_marker_takes_one_word.rs`, `ui/construct_marker_twice.rs`, `ui/construct_marker_on_a_plain_struct.rs` and `ui/marker_qualifier_on_a_field.rs`; `tests/cfg_construct.rs` and the examples, the marker accepted on both containers; the editor fixture's `Marked` and `Defaults`, hover and go-to-definition on the marker bare and under `ulo`.

70. **`is_panic` walks a `Redacted`'s original as `is_permanent` does** (§10.2, Q38-12). Inside a `Redacted` both predicates continue from `original()`: the core's own types by their structure, and any other error down its own `source()` chain, to the one bound of 32 levels. A panic a user's error type wrapped before it was redacted, a constructor returning `Err(MyError { source: lookup })` over a panicked dependency, is found the way a `Permanent` wrapped the same way is, so the two answers agree on one error. The walk reads types and returns a boolean, so the original's text reaches no caller. Tests: `error::wrapped_before_redaction`, a panic and a `Permanent` each under one and under thirty user wrappers found by their predicate, a wrapper holding neither answering `false` to both, and past the bound neither found. The same walk, through a bare error, a `CallError`'s source, a `Redacted`'s original and the errors the core's own types hold where `source()` does not reach them, to the same bound, is `CallError::find::<E>(&BoxError) -> Option<&E>`: a handler reaches a domain error without knowing whether it arrived bare, as a `GuardRejected` does, or inside a `CallError`, as a `Classify` error does. Tests: `error::wrapped_before_redaction::find_walks_a_redacted_original_to_the_same_bound` and `find_reads_the_core_types_as_is_permanent_does` (core), `error::tests::find_reaches_a_bare_error_and_one_inside_a_call_error` and `find_stops_at_the_depth_bound` (transport).

71. **Every struct variant of a public error enum is `#[non_exhaustive]`, in every crate, and a check holds it** (§10.2, Q38-11). The rule items 66 and 67 apply to `StartupError`, `LoadError` and `ConnectError` holds for every public error: a field added to a variant breaks no caller that destructures it. The rule covers every public enum an error holds, reached by any path: the type of a public field or a variant's payload, and the argument and return types of a public inherent method, of the error and of every type reached from it, followed through generic arguments. In the core that is `LookupError`, `WiringError`, `ShutdownFailure`, `FailureReason`, `LoadRefusal`, `InputOrigin`, `ModuleSite` and `Unreached` (through `PrepareEntry::reason`); in the transport crates `ExtractError`, `MetadataError`, `TlsError`, `CertificateCause`, `KeyCause`, `BindError`, `ActivationError`, `CloseCodeError`, `Detail` (through `Details`) and `ulo_rpc::Frame` (through `Codec`'s methods); in `ulo-config` `Origin` (through `Problem::origin`). A variant other crates build gets a constructor named for the variant: `ExtractError::malformed(param, source)`, `Detail::error_info(reason, domain, metadata)`, `Unreached::health_only(service)`, `Origin::file(path, line, column)` and `Frame::req(id, pattern, headers, data)`, with `Frame::input` for `In`, `in` being a keyword. `scripts/error_variants.py` reads each publishable library crate's rustdoc JSON and fails on an unmarked struct variant of a covered enum; CI runs it in the `public reach` job, and it self-tests on a planted unmarked variant on each path of the walk: one an error holds, one reached through a value struct's field, one through a non-error enum's method return and one through a method argument, and a variant held only by a private field, which no caller destructures, is not found.
72. **`TestServer` lives in the core's `ulo::testing`, and the runtime crates supply the runtime** (§11, Q39-9, Q40-6). A test that serves its app needs a bound server, a serving task and a close that answers the `Shutdown`, the three steps `examples/support` writes out for every example. `start_on` uses only the core's API and takes any `ulo::Server`, so it sits beside `TestApp` and `ScriptedTimer`, where every application already has a dependency, and a gRPC-only, WebSocket-only or RPC-only app tests itself without the HTTP backend, `ulo-http-hyper`. The core stays runtime-free because the caller supplies the runtime. The convenience that supplies one mirrors `run`: `ulo_runtime_tokio::testing::start` and `ulo_runtime_smol::testing::start`; the HTTP backend adds `TestServerExt::start(app)`, which names its own `Server`, as an extension trait because the core's type cannot name it, behind its default `tokio` feature. The core method is `start_on` and not `start` so that the extension's `start(app)` is not shadowed by an inherent one. A drop without `close` aborts the serving task, and the port is released only when the runtime next runs it: on an executor nothing runs again, the task and its listener stay. Tests: `crates/ulo/tests/test_server.rs` (a server that is not HTTP served and closed under `test`, `url`, a close after the handle's own, a drop aborting the task, a clock set after the runtime kept, a server binding no address served and closed, an app without a runtime refused at bind), `crates/ulo-runtime-tokio/tests/testing.rs` and `crates/ulo-runtime-smol/tests/testing.rs` (the runtime set, a clock set before `start` replaced, one set after the runtime kept by `start_on`), `crates/ulo-rpc-tcp/tests/test_server.rs` (an RPC app called over TCP in a test whose tree holds no HTTP crate) and `crates/ulo-http-hyper/tests/test_server.rs` (a request served, the port released on close, close after the handle's close and on a drop, the smol listener through `start_on`).
73. **`#[module(identity = type)]` identifies a module with fields by its type, and the module is registered once** (§8.3, Q39-9). A struct with fields is identified by value, which needs `Eq + Hash + Clone`; a root that holds a `Config`, `Clone` and nothing more, has no `#[module]` spelling without the option and implements `Module` by hand. `identity = type` writes `ModuleIdentity::of_type::<Self>()` for it, the same identity a unit module has. Two registrations of a type with other field values would then be one module whose second value goes unused, which is what `DifferingSettings` refuses for settings, and the fields that need the option are the ones that cannot be compared. The module is therefore registered once: the macro marks a struct with fields through `ModuleSettings::__unshared`, and a second registration, by `import` or by `AppHandle::load`, is `WiringError::RegisteredTwice { module, first, second }` with both `ModuleSite`s and the help "a module whose fields are not its identity is registered once; give it an identity (`Eq + Hash + Clone` fields, or `settings()`) to share it". A struct without fields has nothing to differ in and deduplicates as any type-identified module does. `identity` takes `type` and nothing else; any other value is a compile error at the value. Tests: `crates/ulo/tests/wiring_identity_type.rs` (a root holding a value that is not `Eq + Hash` wires, the same module imported twice is refused naming both sites and the help text, three imports are two entries, a field-less module deduplicates, `load` of a registered module is refused), `crates/ulo-codegen-tests/tests/ui/module_identity_unknown_value.rs`; `examples/33-config` is the worked case.
