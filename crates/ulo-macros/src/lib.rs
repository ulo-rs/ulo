//! Macros for `ulo`. Each expands to calls on the core's value-level API, which integration
//! crates call directly; nothing here does what that API cannot.
//!
//! Errors are `syn::Error`s spanned on the offending token, and per-field and per-parameter
//! assertions are emitted with `quote_spanned!`, so a compile error points at the field,
//! parameter or attribute rather than at the macro invocation.

mod construct;
mod construct_attr;
mod enhancers;
mod handlers;
mod module_attr;
mod shared;

use proc_macro::TokenStream;

/// A type the container builds and injects into others: a provider, listed under a module's
/// `providers`.
///
/// On a struct, every field is an injection point except one marked `#[injectable(default)]`,
/// which is set from `Default`. On an impl block, the constructor is the fn named `new`, or the
/// one marked `#[construct]`; its parameters are injection points, it may be `async`, and it may
/// return `Self` or a `Result<Self, E>`. Both marks are read and removed, and a hidden import of
/// each written beside the item lets an editor answer hover and go-to-definition on it. A mark,
/// like the attribute itself, is written bare or under `ulo`, `#[ulo::construct]`; any other
/// qualifier is a compile error at it.
///
/// Arguments: a scope, `singleton`, `execution` or `transient` (none means `Auto`), and
/// `timeout = <Duration expr>`, which writes `CONSTRUCT_TIMEOUT`. `Construct::hooks` is filled by
/// probing the five lifecycle traits.
///
/// A type whose handlers a transport calls is a controller and takes `#[controller]` instead: an
/// `#[injectable]` type is not a `Controller`, so `#[handlers]` on it and `#[module]`'s
/// `controllers` both refuse it at compile time.
#[proc_macro_attribute]
pub fn injectable(attr: TokenStream, item: TokenStream) -> TokenStream {
    shared::attrs::check_invocation("injectable")
        .and_then(|()| construct::expand(construct::Kind::Injectable, attr.into(), item.into()))
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/// A controller: a type the container builds and its transports reach, through the handlers of
/// its `#[handlers]` impl. Only another controller injects one; `wire()` refuses a read of a
/// controller by any other binding.
///
/// It writes what `#[injectable]` writes, from a struct (`#[controller(default)]` marks a field
/// set from `Default`) or from an impl block's `new` or `#[construct]` fn, and takes the same
/// arguments: a scope and `timeout = <Duration expr>`. With no scope it is `Auto`: a singleton if
/// nothing below it needs an execution, and built per call otherwise. It takes no route prefix;
/// the module mounts a controller under one with `m.controller::<T>().at(prefix)`.
///
/// It also writes `impl ulo::Controller`, the mark that makes a type a controller, holding where
/// the type implements `ulo::Handlers`, the impl `#[handlers]` writes, so a `#[controller]` type
/// with no handler impl fails to compile here. `#[module]` accepts under `controllers` only a
/// type implementing `Handlers` and refuses a `Controller` under `providers`, and `wire()` refuses
/// a controller registered as a provider. A WebSocket gateway is a controller and takes
/// `#[controller]` too. A `#[controller]` type whose handlers are mounted by hand, a gateway
/// implementing `Gateway` among them, implements `Handlers` itself.
///
/// It is written bare, `#[controller]`, or as `#[ulo::controller]`; under any other qualifier,
/// a renamed dependency's included, it is a compile error, and a crate depending on `ulo` under
/// another name aliases it, `extern crate framework as ulo;`. The same holds for `#[injectable]`,
/// `#[handlers]` and every marker they read.
///
/// On an impl block it stacks with `#[handlers]` in either order, so the constructor and the
/// handlers can share one impl, `#[controller] #[handlers] impl Greeter { fn new(..) .. }`.
#[proc_macro_attribute]
pub fn controller(attr: TokenStream, item: TokenStream) -> TokenStream {
    shared::attrs::check_invocation("controller")
        .and_then(|()| construct::expand(construct::Kind::Controller, attr.into(), item.into()))
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/// Marks the constructor inside an `#[injectable]` or `#[controller]` impl when it is not named
/// `new`; the attribute on the impl reads and removes it. Anywhere else it is an error.
#[proc_macro_attribute]
pub fn construct(attr: TokenStream, item: TokenStream) -> TokenStream {
    construct_attr::expand(attr.into(), item.into()).unwrap_or_else(syn::Error::into_compile_error).into()
}

/// A module: `imports`, `providers`, `controllers`, `exports` and `global`, lowered to a
/// `Module` impl whose `register` calls the value API. A struct with fields is a configured
/// module, identified by value, and every `Secret<_>` field is registered for redaction.
///
/// Provider entries: `Type` (`provide::<Type>()`), `Type as dyn Trait` (`also_as`), `expr?`
/// (`try_value`), `expr` (`value`), `with = |..| ..` (`with`, or `try_with` when the closure's
/// output is a `Result`), `with(<scope>) = |..| ..` (`singleton`, `execution` or `transient`, or
/// the `try_` form), a bare closure (shorthand for `with = closure`), `K: with = |..| ..` and
/// `K: with(<scope>) = |..| ..` (the same binding, also bound under the trait key `K` with
/// `also_as::<K>(|a| a)`), and `into dyn Trait: [..]` (contributions). A bare path reads as a
/// type, so a constant is bound by value with a block, `{ LIMITS }`. Export entries: `Type` and
/// `reexport Type`. A `controllers` entry implements `Handlers`, and through it `Controller`, by
/// `#[controller]` and `#[handlers]` or by hand, and an entry under `providers`, by type or by
/// value, implements no `Controller`; each mistake is a compile
/// error at the entry. A closure under `providers` is not checked. A controller may be exported,
/// for another controller to read.
///
/// A qualified binding is written with the `@` that reports print for a key. `Type @ Q`
/// (`provide::<Type>().qualified::<Q>()`), `Type as dyn Trait @ Q` (both keys qualified), `expr @ Q`
/// and `expr? @ Q` (`value` and `try_value`, then `.qualified::<Q>()`) take it in `providers`;
/// `Type @ Q` and `reexport Type @ Q` take it in `exports` (`export_qualified`,
/// `reexport_qualified`). A closure binding and an `into` item take none, and a second `@` on an
/// entry is refused:
///
/// ```
/// use ulo::{App, Dep, ExecOptions, injectable, module};
///
/// struct Primary;
/// struct Replica;
///
/// #[derive(Debug, PartialEq)]
/// struct Db(&'static str);
///
/// #[module(
///     providers = [Db("primary") @ Primary, Db("replica") @ Replica],
///     exports = [Db @ Primary, Db @ Replica],
/// )]
/// struct DbModule;
///
/// #[injectable]
/// struct Writer {
///     db: Dep<Db, Primary>,
/// }
///
/// #[module(imports = [DbModule], providers = [Writer])]
/// struct AppModule;
///
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() {
/// let app = App::builder(AppModule).wire().unwrap().connect().await.unwrap();
/// assert_eq!(app.get::<Writer>().await.unwrap().db.0, "primary");
/// # }
/// ```
///
/// `#[module]` writes what a module declares: imports, bindings of a type or a value, collection
/// contributions, controllers and exports. A module that needs a readiness check (`.ready(..)`),
/// a closure hook (`.on_init(..)`, `.on_destroy(..)`), a qualified closure binding or collection,
/// the `enhancer` mark or module metadata such as `ulo_http::PreDispatch` implements `Module` by
/// hand and calls those on the `ModuleDef`; it imports and is imported by `#[module]` modules
/// like any other.
///
/// The key comes first in `K: with = ..` because a closure's body extends as far as it can: in
/// `with = |cfg| RedisCache::new(cfg) as dyn Cache`, the `as` would parse as a cast inside the
/// body. As with `X as dyn T`, the concrete type stays bound in the module too.
///
/// An `into K: [..]` list takes the forms `#[guards]` does, whatever `K` is: a type
/// (`provide::<A>`), `value = expr` (a shared value; the macro writes the `Arc::new`),
/// `with = |..| ..` (a factory written as in `#[guards]`: `with`, or `try_with` when its output
/// is a `Result`) and `with(<scope>) = |..| ..` (`singleton`, `execution` or `transient`, or the
/// `try_` form of each). It also takes `value = expr?` (`try_value`), whose `Err` `wire()`
/// reports as it reports a providers entry's `expr?`. Every item lowers to `contribute::<K>()`.
/// A global enhancer is a contribution under a role key, `into AnyGuard<Http>: [AuthGuard]`,
/// which the core recognises by the key's type.
///
/// A `K` that is module metadata, a type implementing `ulo::Meta` such as `ulo_http::PreDispatch`
/// or `ulo_http::Upgrades`, is not a contribution key: a transport reads it through
/// `m.meta::<K>()`, so `into PreDispatch: [..]` fails to compile with an error naming `K`. Such a
/// module is written by hand with `ModuleDef`.
///
/// `with` says how an item is built: by its closure. How long it lives is a separate argument,
/// written as on `#[injectable]`. `with = ..` declares `Auto`: built once unless what the closure
/// reads needs an execution, then built per execution. A provider contribution under `Auto` is a
/// singleton, and `wire()` refuses one whose closure reads execution data. `with(singleton) = ..`,
/// `with(execution) = ..` and `with(transient) = ..` write the scope instead.
///
/// Inference reads what a closure takes, not what it does. A closure that creates per-call state
/// but reads nothing per call, such as `|| RequestTimer::start()`, is built once under `Auto`;
/// it must be written `with(execution) = || RequestTimer::start()`.
#[proc_macro_attribute]
pub fn module(attr: TokenStream, item: TokenStream) -> TokenStream {
    module_attr::expand(attr.into(), item.into()).unwrap_or_else(syn::Error::into_compile_error).into()
}

/// A controller's handler impl, on every transport: writes `impl ulo::Handlers`, whose `mount`
/// registers each handler, with the impl's `#[guards]`, `#[interceptors]`, `#[error_handlers]` and
/// `#[meta]` as the controller tier and each method's as the method tier; every other item stays
/// as written. A handler is a method carrying its transport's attribute, `#[ulo_http::get(..)]`,
/// `#[ulo_rpc::message(..)]`, `#[ulo_ws::message(..)]` or `#[ulo_grpc::method]`. The controller
/// itself is declared with `#[controller]`, on the struct or stacked on this impl in either order,
/// where `new` or the `#[construct]` fn builds it, and a module that registers the controller
/// mounts these handlers through `Handlers::mount`. `Handlers` requires `Controller`, so on a type
/// that is not a controller, one declared `#[injectable]` among them, `#[handlers]` is a compile
/// error naming `Controller`.
///
/// An impl-level `value = expr` is built once by `Handlers::mount` and shared by every handler
/// it applies to, so a rate limiter declared on the impl limits the controller as a whole. A
/// controller-level transport key that no handler's transport carries, `#[guards(htpp = ..)]`, is
/// a compile error spanned on the key.
///
/// A handler behind `#[cfg]` is mounted, and its transport key counted, only in a build that
/// compiles it. A scoped entry for a transport whose handlers all sit behind
/// `#[cfg(feature = "rpc")]` goes in `#[cfg_attr(feature = "rpc", guards(rpc = ..))]` on the impl,
/// which rustc evaluates before `#[handlers]` reads the impl's attributes.
///
/// `#[handlers]` reads a method's attributes through `cfg_attr`, at any depth:
/// - `#[cfg_attr(feature = "x", ulo_http::get("/beta"))]` makes the method a handler only where
///   the predicate holds, gated like a handler behind `#[cfg]`: mounted, and its key counted, only
///   in that build. Elsewhere it is a plain method, dead code unless something calls it;
///   `#[cfg(feature = "x")]` removes it instead.
/// - `#[cfg_attr(feature = "x", guards(..))]`, and likewise `interceptors`, `error_handlers` and
///   `meta`, on a handler applies its entries only where the predicate holds.
/// - One `cfg_attr` may hold several of these beside unrelated attributes,
///   `#[cfg_attr(feature = "x", ulo_http::get("/beta"), guards(AuthGuard), inline)]`;
///   `#[handlers]` takes the enhancers and `meta` and leaves the rest in place.
///
/// `#[handlers]` reads and removes every enhancer and `#[meta]` attribute it takes, and writes
/// beside the impl a hidden import of each, through which an editor answers hover and
/// go-to-definition on the attribute's name, and on its `ulo` segment when it is written
/// `#[ulo::guards]`. A marker is written bare or under `ulo`; any other qualifier,
/// `#[framework::guards]` or a typo, is a compile error at the attribute.
///
/// A method carrying an attribute outside the language's own (`doc`, `allow`, `cfg`, `inline` and
/// the like), the enhancer and `#[meta]` markers and `#[construct]`, such as
/// `#[tracing::instrument]`, inside `cfg_attr` or not, is treated as a handler, so helpers go in a
/// separate `impl` block. One such attribute written outside any `cfg_attr` makes the method a
/// handler in every build; when every
/// one sits inside a `cfg_attr`, the first decides, so the transport attribute goes first. A
/// transport attribute inside `cfg_attr` beside another attribute macro outside one fails to
/// compile where the predicate fails. A handler takes `&self`, or `self: Arc<Self>` for a reply
/// that outlives the call, and declares no type or const parameter; the controller itself may be
/// generic.
#[proc_macro_attribute]
pub fn handlers(attr: TokenStream, item: TokenStream) -> TokenStream {
    shared::attrs::check_invocation("handlers")
        .and_then(|()| handlers::expand(attr.into(), item.into())).unwrap_or_else(syn::Error::into_compile_error).into()
}

/// Guards for every handler of a `#[handlers]` impl, or for one method: by type (`AuthGuard`),
/// transport-scoped by type (`http = AuthGuard`), by value (`value = expr`), or by closure
/// (`with = |u: Ext<CurrentUser>| ..`). Read and removed by `#[handlers]`, so on the impl it goes
/// below `#[handlers]`; on a method it may sit inside `cfg_attr`; anywhere else it is an error.
///
/// On the impl, an entry applies to every handler and must have the role for each handler's
/// transport; `http = AuthGuard` limits it to that transport's handlers. `value = expr` is built
/// once and shared: on a method by that handler's calls, on the impl by every handler it applies
/// to.
///
/// A closure is written synchronously, its parameters injection points. `with` says only that
/// the closure builds the guard; how long the guard lives is a separate argument, written as on
/// `#[injectable]`. `with = ..` declares `Auto`: built once and shared unless what the closure
/// reads needs an execution, then built per execution. `with(singleton) = ..`,
/// `with(execution) = ..` and `with(transient) = ..` write the scope instead, and
/// `http(with(execution) = ..)` limits one to a transport's handlers.
///
/// Inference reads what a closure takes, not what it does. A closure that creates per-call state
/// but reads nothing per call, such as `|| RequestTimer::start()`, is built once under `Auto`;
/// it must be written `with(execution) = || RequestTimer::start()`.
#[proc_macro_attribute]
pub fn guards(attr: TokenStream, item: TokenStream) -> TokenStream {
    enhancers::marker("guards", attr.into(), item.into()).unwrap_or_else(syn::Error::into_compile_error).into()
}

/// Interceptors, with the same forms as `#[guards]`.
#[proc_macro_attribute]
pub fn interceptors(attr: TokenStream, item: TokenStream) -> TokenStream {
    enhancers::marker("interceptors", attr.into(), item.into()).unwrap_or_else(syn::Error::into_compile_error).into()
}

/// Error handlers, with the same forms as `#[guards]`.
#[proc_macro_attribute]
pub fn error_handlers(attr: TokenStream, item: TokenStream) -> TokenStream {
    enhancers::marker("error_handlers", attr.into(), item.into()).unwrap_or_else(syn::Error::into_compile_error).into()
}

/// Declared metadata on a handler or on its `#[handlers]` impl: plain `Send + Sync + 'static`
/// values, built once when the handler mounts, which guards and interceptors read through
/// `cx.exec().handler()`. The method's declaration of a type wins over the impl's. Read and
/// removed by `#[handlers]`, so on the impl it goes below `#[handlers]`; on a method it may sit inside
/// `cfg_attr`; anywhere else it is an error.
#[proc_macro_attribute]
pub fn meta(attr: TokenStream, item: TokenStream) -> TokenStream {
    enhancers::meta_marker(attr.into(), item.into()).unwrap_or_else(syn::Error::into_compile_error).into()
}

/// Carries a handler's controller-tier and method-tier enhancers and metadata from `#[handlers]` to
/// the transport's handler attribute, which consumes it (`ulo_handler_codegen::protocol`).
/// Reaching expansion means no transport attribute consumed it.
#[doc(hidden)]
#[proc_macro_attribute]
pub fn __handler(attr: TokenStream, item: TokenStream) -> TokenStream {
    enhancers::unconsumed_handler(attr.into(), item.into()).unwrap_or_else(syn::Error::into_compile_error).into()
}

/// Invoked by a transport's generated mount function with its transport type, its transport key,
/// the name of its `shared` parameter and the tokens `__handler` carried: expands to the two
/// `EnhancerSpec`s for one handler, with one role assertion per enhancer naming the handler.
#[doc(hidden)]
#[proc_macro]
pub fn __enhancer_specs(input: TokenStream) -> TokenStream {
    enhancers::specs(input.into()).unwrap_or_else(syn::Error::into_compile_error).into()
}
