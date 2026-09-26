# 0058 — A provider declaration is a value

Status: proposed

A provider is declared by an expression: three constructors, a type's own declaration, and two
modifiers that compose over all of them. A token in a declaration is a value implementing
`IntoToken`, never a spelling a macro reads. An enhancer role comes from the enhancer's own type or
from the token's. A provider factory is always async. `#[new]` is the one constructor. What `build`
returns is a `Registration`.

## Context

**Two vocabularies over ten code paths.** A provider is declared by `provide!` with five markers, or
by one of `provider_value!`, `provider_factory!`, `provider_alias!` and `provider_token!`. They are
not alternatives: `provide!` classifies its input, renders it back to tokens and hands it to the
other four. Under both, `multi` is a second copy of every declaration kind rather than a property of
a binding, so five acts by two channels are ten code paths.

**A token is read by its spelling.** One function decides whether a token path names a type or a
const by asking whether it has one segment and is written in capitals. A `Token<T>` const in a
`tokens` module, where a project puts them, is refused in every macro position, and a type named
`HTTP` is read as a const. Each surface refuses a different subset of spellings. The refusals are
`E0573`, `E0423` and `E0747`, and none names the constraint. `provide!` carries a second, looser
classifier whose answer the handler it forwards to discards.

**The module grammar has four dialects.** `#[module]`'s `imports:` and `providers:` take an
expression; `controllers:` and `exports:` take a bare identifier. A module cannot export a
string-token provider, a generic provider or a provider from a submodule, and cannot declare a
path-qualified controller. `DynamicModule`'s builder accepts what the attribute refuses, and splits
each list into a by-type method and a by-factory method. `Extension<T>` is declared in `providers:`
through a factory registered per payload type, and `exports:` cannot name it without a type alias.

**Construction has two mechanisms that read different places.** `#[new]` reads a method's
parameters. `init = "…"` reads the struct's `#[inject]` fields and passes them positionally, so the
same struct moved between the two forms resolves different dependencies. `#[default(expr)]` is inert
whenever `#[new]` is present, with no warning. `#[controller]` refuses `init = "…"` already, in a
message saying "as with `#[injectable]`", which is false while `#[injectable]` accepts it. An init
method named `from_request` changes what the macro emits.

**A factory is sync or async, and the duality is copied.** Nest's `useFactory` accepts a value or a
promise because JavaScript's `await` on a non-promise is a no-op. Rust has no such affordance.
Building it by hand has two shapes and both fail: a marker type parameter is ambiguous at the call
site for an async closure (`E0282`), and two entry points accept the async form through the sync
door as a provider of futures.

**A role is detected where the value's type is concrete.** `provider_value!` and
`provider_factory!` emit autoref probes that register a middleware, guard, interceptor or
error-handler role when the produced value's type implements one. A value surface is a generic
function, which sees its value only through its bounds, so the probe has nothing to resolve against
there.

## Decision

**The surface is values.**

```rust
providers: [
    Db,                                               // the type declares itself
    Provide::value("API_KEY", key),
    Provide::factory("POOL", async |c: Config| open(c).await),
    Provide::alias("LOG", "LOGGER"),
    FileLogger::provide().under("LOGGER"),
    A::provide().multi::<dyn Plugin>("PLUGINS"),
    B::provide().multi::<dyn Plugin>("PLUGINS"),
]
```

Three constructors, one accessor, two modifiers. `under` and `multi` are one blanket extension over
`ProviderFactory`, so they compose over every constructor and over a type's own declaration, and they
chain. `multi` is a property of a binding, not a kind. A factory's dependencies come from its
parameter types through a trait implemented per arity, not from the closure's text. `provide!` and
the four macros are deleted.

**A token is an `impl IntoToken`.** In value position every spelling works: a qualified const, a bare
const, a runtime `String`. Nothing classifies a path; the compiler rejects a value that is neither a
`Token<T>` nor a `&str`, and `#[diagnostic::on_unimplemented]` carries the message. A typed token is
load-bearing: `Provide::value(tokens::API_KEY, 42u32)` where `API_KEY: Token<String>` fails to
compile, and the string form accepts it.

**A role comes from a type: the enhancer's own, or the token's.** An `#[injectable]` enhancer
registers its roles, and `.under(token)` forwards them. A token typed with a role trait,
`Token<dyn Guard<HttpContext>>`, makes whatever is provided under it take that role, and the
constructor's bound refuses a value that does not implement it. `APP_GUARD` and `APP_INTERCEPTOR`
are role tokens: `APP_GUARD` is a `Token<dyn Guard<HttpContext>>`. A string token, or a token typed with a data type, carries data and registers no
role.

```rust
pub const AUTH: Token<dyn Guard<HttpContext>> = Token::new("AUTH_GUARD");

providers: [
    Provide::value(AUTH, HeaderGuard("x-auth")),       // a guard, checked
    Provide::value(APP_GUARD, RateLimit::new(100)),    // a global guard, checked
    Provide::value("PORT", 3000u16),                   // data
]
```

**`#[module]` and the builder take the same expressions.** All four keys parse expressions, and
`exports:` takes both forms the builder's export methods have. The builder has one
`.provider(value)` and one `.controller(value)`. The attribute and the builder are two syntaxes for
one list. An `Extension<T>` needs no declaration, as `Extensions` needs none: the container answers
any `Extension<T>` token from the execution's bag.

**A provider factory is always async.** The bound is `F: Fn(A..) -> Fut, Fut: Future<Output = R> +
Send`, which accepts every async spelling and refuses a sync closure. `AsyncFn` accepts the same set,
but its future type is unstable, so `Send` cannot be named on it. `Provider::resolve` and
`ProviderFactory::build` are already async; the surface becomes consistent with the SPI.

The rule behind this one: *where a language affordance is being copied rather than a design, check
that the affordance exists.* Nest's duality rests on `await`, and copying it means building `await`'s
tolerance by hand.

**`#[new]` is the only constructor.** It reads dependencies from the signature, where a Rust reader
looks, and needs no attribute on the fields. `init = "…"` is removed from `#[injectable]`, and
`#[controller]`'s refusal message goes with the key. `#[default]` beside `#[new]` is refused naming
both, since the constructor decides every field.

**One word means one thing, and a type is named for what its consumer does with it.**

| Concept | Name |
| --- | --- |
| Recipe: token, dependencies, build | `ProviderFactory` |
| What answers a resolution | `Provider` |
| What `build` returns: an instance and its roles | `Registration` |
| Marking a type a provider | `#[injectable]` |
| Marking a dependency | `#[inject]` |
| A type's own declaration | `T::provide()` |
| Binding under another token | `.under(token)` |
| Contributing to a collected token | `.multi::<Tr>(base)` |

`provide()` is a trait method, so `DeclaresProvider` goes in the prelude with the modifiers.

**Nothing is kept for compatibility.** The crate is unpublished and its first release will be a
beta. The macros are deleted rather than deprecated, `init = "…"` is deleted without a migration error,
`Injectable` is renamed without an alias, and `__ulo_provider_factory` leaves the public surface.

## Consequences

- Every provider declaration in the tree, and every page showing `provide!` or one of the four
  macros, is rewritten.
- A token spelled as a qualified const, a bare const or a runtime string works in every position.
- A module can export a string-token provider, a generic provider and a provider from a submodule,
  and declare a path-qualified controller.
- A sync factory gains one word, `async`.
- A struct's dependencies have one source, its `#[new]` signature.
- A value becomes an enhancer only where a type says so. A plain guard value provided under a
  string token registers no role, and a `#[use_guards]` naming that token fails startup with the
  registry's not-found diagnostic. A value that does not implement `Guard` fails to compile under a
  guard token.

## Roads not taken

**One grammar for the five markers.** It leaves the ten code paths and the token as a token tree.

**Both vocabularies with one documented as preferred.** That is the state today, and the tree's
call sites are split across both spellings.

**Fixing each token surface as it is reported.** It leaves five surfaces with five subsets, and the
defect has been rediscovered from two directions already.

**Documenting the module grammar's limits.** It sends an application needing a string-token export
to `DynamicModule`.

**A marker type parameter over sync and async factories**, ambiguous at the call site for an async
closure, and **two entry points**, which compiled a provider of futures until an `Output: Clone`
bound was added and still leaves two doors for one act.

**`init = "…"` kept beside `#[new]`**, which keeps two sources of dependencies for one struct.

**Detecting a value's role inside the constructor.** Stable Rust cannot ask a generic value
whether it implements a trait. **Naming the role at each declaration** (`.guard::<Http>()`) adds a
modifier per role where the token already says it. **A `#[module]` syntax of its own**, emitting the
same probes, would work in one of the two places a declaration is written and leave the builder
without roles.
