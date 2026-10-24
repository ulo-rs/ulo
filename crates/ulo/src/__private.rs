//! Support for the code `ulo-macros` generates. Not part of the public API: names and shapes here
//! change with the macros.

pub use std::sync::Arc;

pub use ulo_macros::{__enhancer_specs, __handler};

use std::marker::PhantomData;

use crate::construct::ConstructError;
use crate::dependency::{Dependencies, DependencyLabel, FromContainer};
use crate::scope::{AllowedIn, Scope};
use crate::timer::BoxError;
use crate::transport::Transport;
use crate::transport::metadata::TransportMetadata;

/// One injection point of type `S` in a construct of scope `Sc`, as the code `#[injectable]`
/// generates declares it. The generated `dependencies` calls `__ulo_field` or `__ulo_param` on
/// `(&Site::<S, Sc>::new())` with [`Declare`] and [`Declined`] in scope, which method resolution
/// settles on whether `S: FromContainer`.
///
/// Every probe method in this module, and in each transport crate's `__private`, carries the
/// `__ulo_` prefix. The probe call is method syntax in the user's module, where a user trait in
/// scope with a method of the same name, implemented for every type, is a candidate at the same
/// autoref step as the probe's arm: the call is then ambiguous (E0034), or, where the arm's bounds
/// do not hold, resolves to the user's method.
///
/// A type that is not `FromContainer` takes the `Declined` arm and declares nothing. The refusal
/// for it is the read in `construct`, which names `FromContainer` itself; declaring a second time
/// would report the same missing impl once per function, and the scope check below would add an
/// error about a scope the field's type never reached. The read is bounded in the same impl as
/// the declaration, so a type that takes the `Declined` arm does not compile and never runs.
pub struct Site<S, Sc>(PhantomData<fn() -> (S, Sc)>);

impl<S, Sc> Site<S, Sc> {
    #[allow(clippy::new_without_default)]
    pub const fn new() -> Self {
        Site(PhantomData)
    }
}

/// The arm for a `FromContainer` type. Its scope bound is on the method, so it is checked, and
/// reported at the field, only for a type that got this far: an `Ext` in an explicit singleton.
pub trait Declare<S, Sc: Scope> {
    fn __ulo_field(&self, d: &mut Dependencies, name: &'static str)
    where
        S: AllowedIn<Sc>;

    fn __ulo_param(&self, d: &mut Dependencies, name: &'static str)
    where
        S: AllowedIn<Sc>;
}

impl<S: FromContainer, Sc: Scope> Declare<S, Sc> for Site<S, Sc> {
    fn __ulo_field(&self, d: &mut Dependencies, name: &'static str)
    where
        S: AllowedIn<Sc>,
    {
        d.field::<S>(name);
    }

    fn __ulo_param(&self, d: &mut Dependencies, name: &'static str)
    where
        S: AllowedIn<Sc>,
    {
        d.param::<S>(name);
    }
}

/// The arm for a type that is not `FromContainer`; reached through one more autoref than
/// [`Declare`], so it applies only when that one does not.
pub trait Declined<S, Sc> {
    fn __ulo_field(&self, _d: &mut Dependencies, _name: &'static str) {}

    fn __ulo_param(&self, _d: &mut Dependencies, _name: &'static str) {}
}

impl<S, Sc> Declined<S, Sc> for &Site<S, Sc> {}

/// How a wiring report names a handler parameter: by the identifier its signature writes, or, for
/// a destructuring pattern, by its position among the parameters after the receiver, from 0, which
/// the report prints from 1 as `param #n`.
#[derive(Clone, Copy, Debug)]
pub enum HandlerParam {
    Named(&'static str),
    At(usize),
}

/// Everything `declare` adds to `d`, labelled as the handler parameter `param` whatever label
/// `declare` gave it: a transport's handler attribute declares each parameter's reads through
/// this, so a parameter built from the call names its container reads as one built from the
/// container does.
pub fn handler_param(d: &mut Dependencies, param: HandlerParam, declare: impl FnOnce(&mut Dependencies)) {
    let start = d.list.len();
    declare(d);
    let label = match param {
        HandlerParam::Named(name) => DependencyLabel::Param(name),
        HandlerParam::At(index) => DependencyLabel::Position(index),
    };
    for record in &mut d.list[start..] {
        record.label = label;
    }
}

/// Whether `key` is one of `keys`, byte for byte: the assertion `#[routes]` emits for each
/// controller-level transport-scoped enhancer key against the `__ULO_KEY_<name>` constants its
/// handlers' transport attributes emit (transports DESIGN §2.1, X1, X11). A loop over bytes,
/// since a trait method such as `PartialEq::eq` is not callable in a `const fn` on stable.
pub const fn key_in(key: &str, keys: &[&str]) -> bool {
    let mut i = 0;
    while i < keys.len() {
        if bytes_eq(key.as_bytes(), keys[i].as_bytes()) {
            return true;
        }
        i += 1;
    }
    false
}

const fn bytes_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
}

/// The impl-level `value` entries of one `#[routes]` impl, each built once by the generated
/// `Controller::mount` into an `Arc` and handed to every handler's mount function (transports
/// DESIGN §2.1, X2). `T` is a tuple `(Arc<V0>, Arc<V1>, ..)`, one element per `value` entry in the
/// order written across the impl's three enhancer attributes, entries scoped to other transports
/// included; `()` when the impl has none.
///
/// A handler's mount function takes `&Shared<(Arc<V0>, ..)>` with one type parameter per entry,
/// bounding by its transport's role only the entries that apply to it, and registers each with
/// `EnhancerSpec::{guard,interceptor,error_handler}_arc(Arc::clone(&shared.0.N))`.
pub struct Shared<T>(pub T);

/// The `#[meta]` transport probe every transport's handler attribute emits once per metadata
/// value, at either tier, spanned at the value and under its gates (transports DESIGN §2.5, X24):
///
/// ```text
/// let (): () = (&&&::ulo::__private::MetaProbe::<::ulo_http::Http, _>::new(&value)).__ulo_check();
/// ```
///
/// with `MetaSame`, `MetaOther` and `MetaAny` imported anonymously. Three arms, each one reference
/// deeper than the priority reads, since method lookup tries the impl on `&&MetaProbe` first,
/// then `&MetaProbe`, then the bare type, and the first whose impl where-clauses hold wins:
///
/// - a value whose [`TransportMetadata::Transport`](crate::TransportMetadata) is `T`, on
///   `&&MetaProbe`, answers `()`;
/// - a value naming another transport `U`, on `&MetaProbe`, answers [`MetaMismatch<T, U>`], which
///   the `let (): ()` refuses with E0308 at the value, both transports in the message;
/// - a value implementing no `TransportMetadata`, on `MetaProbe`, answers `()`.
///
/// The handler's transport is a parameter of the probe type rather than of the method, since
/// lookup checks an impl's where-clauses and never a method's.
pub struct MetaProbe<T, V: ?Sized> {
    _t: PhantomData<fn() -> T>,
    _v: PhantomData<fn() -> *const V>,
}

impl<T, V: ?Sized> MetaProbe<T, V> {
    pub fn new(_value: &V) -> Self {
        MetaProbe { _t: PhantomData, _v: PhantomData }
    }
}

/// What the probe answers for a metadata value of transport `U` written on a handler of
/// transport `T`. It is never built: the `let (): ()` the probe sits in refuses the type, and the
/// type's name is the diagnostic, `expected (), found MetaMismatch<Http, Rpc>`.
pub struct MetaMismatch<T, U> {
    _t: PhantomData<fn() -> (T, U)>,
}

/// The first arm: the value's transport is the handler's.
pub trait MetaSame {
    fn __ulo_check(&self);
}

impl<T: Transport, V: TransportMetadata<Transport = T>> MetaSame for &&MetaProbe<T, V> {
    fn __ulo_check(&self) {}
}

/// The second arm: the value names another transport.
pub trait MetaOther<T, U> {
    fn __ulo_check(&self) -> MetaMismatch<T, U>;
}

impl<T: Transport, V: TransportMetadata> MetaOther<T, V::Transport> for &MetaProbe<T, V> {
    fn __ulo_check(&self) -> MetaMismatch<T, V::Transport> {
        MetaMismatch { _t: PhantomData }
    }
}

/// The third arm: the value names no transport and is valid on every one.
pub trait MetaAny {
    fn __ulo_check(&self);
}

impl<T, V: ?Sized> MetaAny for MetaProbe<T, V> {
    fn __ulo_check(&self) {}
}

/// What a constructor returns, `Self` or `Result<Self, E>`, decided by type rather than by the
/// spelling of the return type, so an alias for a `Result` is read as one.
#[diagnostic::on_unimplemented(
    message = "a constructor returns `{T}` or `Result<{T}, E>`, not `{Self}`",
    label = "returned by this constructor",
    note = "the error type `E` must convert into `BoxError`, as any `std::error::Error + Send + Sync` does"
)]
pub trait IntoConstructed<T> {
    fn into_constructed(self) -> Result<T, ConstructError>;
}

impl<T> IntoConstructed<T> for T {
    fn into_constructed(self) -> Result<T, ConstructError> {
        Ok(self)
    }
}

impl<T, E: Into<BoxError>> IntoConstructed<T> for Result<T, E> {
    fn into_constructed(self) -> Result<T, ConstructError> {
        self.map_err(ConstructError::failed)
    }
}

/// The autoref probes behind `hooks!` and `#[injectable]`'s generated `Construct::hooks`:
/// `(&Probe::of(h)).__ulo_register_init(h)` reaches `RegisterInit` when the concrete type
/// implements `OnModuleInit` and falls back, one autoref further, to the no-op `NoInit` otherwise.
pub mod hooks {
    use std::marker::PhantomData;

    use crate::hooks::{
        BeforeApplicationShutdown, Hooks, OnApplicationBootstrap, OnApplicationShutdown, OnModuleDestroy,
        OnModuleInit,
    };

    pub struct Probe<T>(PhantomData<fn() -> T>);

    impl<T> Probe<T> {
        pub fn of(_hooks: &Hooks<T>) -> Self {
            Probe(PhantomData)
        }
    }

    pub trait RegisterInit<T> {
        fn __ulo_register_init(&self, h: &mut Hooks<T>);
    }
    impl<T: OnModuleInit> RegisterInit<T> for Probe<T> {
        fn __ulo_register_init(&self, h: &mut Hooks<T>) {
            h.on_module_init();
        }
    }
    pub trait NoInit<T> {
        fn __ulo_register_init(&self, _h: &mut Hooks<T>) {}
    }
    impl<T> NoInit<T> for &Probe<T> {}

    pub trait RegisterBootstrap<T> {
        fn __ulo_register_bootstrap(&self, h: &mut Hooks<T>);
    }
    impl<T: OnApplicationBootstrap> RegisterBootstrap<T> for Probe<T> {
        fn __ulo_register_bootstrap(&self, h: &mut Hooks<T>) {
            h.on_application_bootstrap();
        }
    }
    pub trait NoBootstrap<T> {
        fn __ulo_register_bootstrap(&self, _h: &mut Hooks<T>) {}
    }
    impl<T> NoBootstrap<T> for &Probe<T> {}

    pub trait RegisterDestroy<T> {
        fn __ulo_register_destroy(&self, h: &mut Hooks<T>);
    }
    impl<T: OnModuleDestroy> RegisterDestroy<T> for Probe<T> {
        fn __ulo_register_destroy(&self, h: &mut Hooks<T>) {
            h.on_module_destroy();
        }
    }
    pub trait NoDestroy<T> {
        fn __ulo_register_destroy(&self, _h: &mut Hooks<T>) {}
    }
    impl<T> NoDestroy<T> for &Probe<T> {}

    pub trait RegisterBeforeShutdown<T> {
        fn __ulo_register_before_shutdown(&self, h: &mut Hooks<T>);
    }
    impl<T: BeforeApplicationShutdown> RegisterBeforeShutdown<T> for Probe<T> {
        fn __ulo_register_before_shutdown(&self, h: &mut Hooks<T>) {
            h.before_application_shutdown();
        }
    }
    pub trait NoBeforeShutdown<T> {
        fn __ulo_register_before_shutdown(&self, _h: &mut Hooks<T>) {}
    }
    impl<T> NoBeforeShutdown<T> for &Probe<T> {}

    pub trait RegisterShutdown<T> {
        fn __ulo_register_shutdown(&self, h: &mut Hooks<T>);
    }
    impl<T: OnApplicationShutdown> RegisterShutdown<T> for Probe<T> {
        fn __ulo_register_shutdown(&self, h: &mut Hooks<T>) {
            h.on_application_shutdown();
        }
    }
    pub trait NoShutdown<T> {
        fn __ulo_register_shutdown(&self, _h: &mut Hooks<T>) {}
    }
    impl<T> NoShutdown<T> for &Probe<T> {}
}

/// The autoref probe `#[module]` writes at a factory's call site, for a closure entry of a
/// providers list and for a `with` entry of an `into` list: a closure whose future outputs a
/// `Result` reaches the `try_` registration, any other the plain one. The value API keeps two methods
/// because one method cannot serve both outputs.
///
/// `Args` is a parameter of `Probe` rather than of the methods: method probing checks an impl's
/// where-clauses, which is what lets the ranking fall through, and it never checks a method's.
pub mod factory {
    use std::cell::Cell;
    use std::marker::PhantomData;
    use std::sync::Arc;

    use crate::binding::contribute::Contribute;
    use crate::binding::factory::Factory;
    use crate::module::def::ModuleDef;
    use crate::scope::{Auto, PerExecution, Singleton, Transient};
    use crate::timer::BoxError;

    /// The factory sits in a `Cell` because the probe methods take `&self`, which the autoref
    /// ranking needs, and registration moves the factory into the binding.
    pub struct Probe<F, Args> {
        factory: Cell<Option<F>>,
        _a: PhantomData<fn() -> Args>,
    }

    impl<F, Args> Probe<F, Args> {
        pub fn new(factory: F) -> Self {
            Probe { factory: Cell::new(Some(factory)), _a: PhantomData }
        }
    }

    /// The providers arm: a single binding declared by closure, `with = ..`, `with(<scope>) = ..`
    /// or a bare closure, in the scope `S` (`Auto` for the first and the last). `__ulo_bind_as` is
    /// the key-first form `K: with = ..`, which also binds the output under `K` through the
    /// coercion closure written at the expansion, where `Arc<Built>` and `Arc<K>` are concrete.
    ///
    /// `#[track_caller]` on every method carries the providers entry's location through to the
    /// binding record, which the wiring errors print.
    pub trait FallibleBinding {
        type Built: Send + Sync + 'static;

        #[track_caller]
        fn __ulo_bind<S: ProviderScope>(&self, m: &mut ModuleDef<'_>);

        #[track_caller]
        fn __ulo_bind_as<K, S, C>(&self, m: &mut ModuleDef<'_>, coerce: C)
        where
            K: ?Sized + Send + Sync + 'static,
            S: ProviderScope,
            C: Fn(Arc<Self::Built>) -> Arc<K> + Send + Sync + 'static;
    }
    impl<F, Args, T, E> FallibleBinding for Probe<F, Args>
    where
        F: Factory<Args, Output = Result<T, E>>,
        T: Send + Sync + 'static,
        E: Into<BoxError> + Send + 'static,
    {
        type Built = T;

        #[track_caller]
        fn __ulo_bind<S: ProviderScope>(&self, m: &mut ModuleDef<'_>) {
            if let Some(factory) = self.factory.take() {
                S::try_bind::<Args, F, T, E>(m, factory);
            }
        }

        #[track_caller]
        fn __ulo_bind_as<K, S, C>(&self, m: &mut ModuleDef<'_>, coerce: C)
        where
            K: ?Sized + Send + Sync + 'static,
            S: ProviderScope,
            C: Fn(Arc<Self::Built>) -> Arc<K> + Send + Sync + 'static,
        {
            if let Some(factory) = self.factory.take() {
                S::try_bind_as::<K, Args, F, T, E, C>(m, factory, coerce);
            }
        }
    }

    pub trait PlainBinding {
        type Built: Send + Sync + 'static;

        #[track_caller]
        fn __ulo_bind<S: ProviderScope>(&self, m: &mut ModuleDef<'_>);

        #[track_caller]
        fn __ulo_bind_as<K, S, C>(&self, m: &mut ModuleDef<'_>, coerce: C)
        where
            K: ?Sized + Send + Sync + 'static,
            S: ProviderScope,
            C: Fn(Arc<Self::Built>) -> Arc<K> + Send + Sync + 'static;
    }
    impl<F, Args> PlainBinding for &Probe<F, Args>
    where
        F: Factory<Args>,
        F::Output: Send + Sync + 'static,
    {
        type Built = <F as Factory<Args>>::Output;

        #[track_caller]
        fn __ulo_bind<S: ProviderScope>(&self, m: &mut ModuleDef<'_>) {
            if let Some(factory) = self.factory.take() {
                S::bind::<Args, F>(m, factory);
            }
        }

        #[track_caller]
        fn __ulo_bind_as<K, S, C>(&self, m: &mut ModuleDef<'_>, coerce: C)
        where
            K: ?Sized + Send + Sync + 'static,
            S: ProviderScope,
            C: Fn(Arc<Self::Built>) -> Arc<K> + Send + Sync + 'static,
        {
            if let Some(factory) = self.factory.take() {
                S::bind_as::<K, Args, F, C>(m, factory, coerce);
            }
        }
    }

    /// A scope marker as a providers entry's `with` names it, mapped to the `ModuleDef` method
    /// that registers a closure in that scope: `Auto` to `with`, `Singleton` to `singleton`,
    /// `PerExecution` to `execution`, `Transient` to `transient`, each with its `try_` twin.
    pub trait ProviderScope {
        #[track_caller]
        fn bind<Args, F>(m: &mut ModuleDef<'_>, factory: F)
        where
            F: Factory<Args>,
            F::Output: Send + Sync + 'static;

        #[track_caller]
        fn try_bind<Args, F, T, E>(m: &mut ModuleDef<'_>, factory: F)
        where
            F: Factory<Args, Output = Result<T, E>>,
            T: Send + Sync + 'static,
            E: Into<BoxError> + Send + 'static;

        #[track_caller]
        fn bind_as<K, Args, F, C>(m: &mut ModuleDef<'_>, factory: F, coerce: C)
        where
            K: ?Sized + Send + Sync + 'static,
            F: Factory<Args>,
            F::Output: Send + Sync + 'static,
            C: Fn(Arc<F::Output>) -> Arc<K> + Send + Sync + 'static;

        #[track_caller]
        fn try_bind_as<K, Args, F, T, E, C>(m: &mut ModuleDef<'_>, factory: F, coerce: C)
        where
            K: ?Sized + Send + Sync + 'static,
            F: Factory<Args, Output = Result<T, E>>,
            T: Send + Sync + 'static,
            E: Into<BoxError> + Send + 'static,
            C: Fn(Arc<T>) -> Arc<K> + Send + Sync + 'static;
    }

    macro_rules! provider_scope {
        ($($marker:ty => $plain:ident, $fallible:ident;)*) => {$(
            impl ProviderScope for $marker {
                #[track_caller]
                fn bind<Args, F>(m: &mut ModuleDef<'_>, factory: F)
                where
                    F: Factory<Args>,
                    F::Output: Send + Sync + 'static,
                {
                    m.$plain::<Args, F>(factory);
                }

                #[track_caller]
                fn try_bind<Args, F, T, E>(m: &mut ModuleDef<'_>, factory: F)
                where
                    F: Factory<Args, Output = Result<T, E>>,
                    T: Send + Sync + 'static,
                    E: Into<BoxError> + Send + 'static,
                {
                    m.$fallible::<Args, F, T, E>(factory);
                }

                #[track_caller]
                fn bind_as<K, Args, F, C>(m: &mut ModuleDef<'_>, factory: F, coerce: C)
                where
                    K: ?Sized + Send + Sync + 'static,
                    F: Factory<Args>,
                    F::Output: Send + Sync + 'static,
                    C: Fn(Arc<F::Output>) -> Arc<K> + Send + Sync + 'static,
                {
                    m.$plain::<Args, F>(factory).also_as::<K>(coerce);
                }

                #[track_caller]
                fn try_bind_as<K, Args, F, T, E, C>(m: &mut ModuleDef<'_>, factory: F, coerce: C)
                where
                    K: ?Sized + Send + Sync + 'static,
                    F: Factory<Args, Output = Result<T, E>>,
                    T: Send + Sync + 'static,
                    E: Into<BoxError> + Send + 'static,
                    C: Fn(Arc<T>) -> Arc<K> + Send + Sync + 'static,
                {
                    m.$fallible::<Args, F, T, E>(factory).also_as::<K>(coerce);
                }
            }
        )*};
    }

    provider_scope! {
        Auto => with, try_with;
        Singleton => singleton, try_singleton;
        PerExecution => execution, try_execution;
        Transient => transient, try_transient;
    }

    /// The `into` arm: a contribution to `U` declared by closure, in the scope `S`, which is
    /// `Auto` for `with = ..` and the written marker for `with(<scope>) = ..`. The coercion
    /// closure has to be written in the expansion, where `Arc<Built>` and `Arc<U>` are concrete
    /// types and the one unsizes to the other; a generic body cannot unsize. The expansion cannot
    /// name the built type, so `Built` is an associated type, resolved once the ranking has
    /// picked the arm.
    pub trait FallibleContribution {
        type Built: Send + Sync + 'static;

        #[track_caller]
        fn __ulo_contribute_with<U, S, C>(&self, m: &mut ModuleDef<'_>, coerce: C)
        where
            U: ?Sized + Send + Sync + 'static,
            S: ContributionScope,
            C: Fn(Arc<Self::Built>) -> Arc<U> + Send + Sync + 'static;
    }
    impl<F, Args, T, E> FallibleContribution for Probe<F, Args>
    where
        F: Factory<Args, Output = Result<T, E>>,
        T: Send + Sync + 'static,
        E: Into<BoxError> + Send + 'static,
    {
        type Built = T;

        #[track_caller]
        fn __ulo_contribute_with<U, S, C>(&self, m: &mut ModuleDef<'_>, coerce: C)
        where
            U: ?Sized + Send + Sync + 'static,
            S: ContributionScope,
            C: Fn(Arc<Self::Built>) -> Arc<U> + Send + Sync + 'static,
        {
            if let Some(factory) = self.factory.take() {
                S::try_contribute::<U, Args, F, T, E, C>(m.contribute::<U>(), factory, coerce);
            }
        }
    }

    pub trait PlainContribution {
        type Built: Send + Sync + 'static;

        #[track_caller]
        fn __ulo_contribute_with<U, S, C>(&self, m: &mut ModuleDef<'_>, coerce: C)
        where
            U: ?Sized + Send + Sync + 'static,
            S: ContributionScope,
            C: Fn(Arc<Self::Built>) -> Arc<U> + Send + Sync + 'static;
    }
    impl<F, Args> PlainContribution for &Probe<F, Args>
    where
        F: Factory<Args>,
        F::Output: Send + Sync + 'static,
    {
        type Built = <F as Factory<Args>>::Output;

        #[track_caller]
        fn __ulo_contribute_with<U, S, C>(&self, m: &mut ModuleDef<'_>, coerce: C)
        where
            U: ?Sized + Send + Sync + 'static,
            S: ContributionScope,
            C: Fn(Arc<Self::Built>) -> Arc<U> + Send + Sync + 'static,
        {
            if let Some(factory) = self.factory.take() {
                S::contribute::<U, Args, F, C>(m.contribute::<U>(), factory, coerce);
            }
        }
    }

    /// A scope marker as an `into` entry's `with` names it, mapped to the `Contribute` method
    /// that registers a closure in that scope: `Auto` to `with`, `Singleton` to `singleton`,
    /// `PerExecution` to `execution`, `Transient` to `transient`, each with its `try_` twin.
    pub trait ContributionScope {
        #[track_caller]
        fn contribute<U, Args, F, C>(into: Contribute<'_, U>, factory: F, coerce: C)
        where
            U: ?Sized + Send + Sync + 'static,
            F: Factory<Args>,
            F::Output: Send + Sync + 'static,
            C: Fn(Arc<F::Output>) -> Arc<U> + Send + Sync + 'static;

        #[track_caller]
        fn try_contribute<U, Args, F, T, E, C>(into: Contribute<'_, U>, factory: F, coerce: C)
        where
            U: ?Sized + Send + Sync + 'static,
            F: Factory<Args, Output = Result<T, E>>,
            T: Send + Sync + 'static,
            E: Into<BoxError> + Send + 'static,
            C: Fn(Arc<T>) -> Arc<U> + Send + Sync + 'static;
    }

    macro_rules! contribution_scope {
        ($($marker:ty => $plain:ident, $fallible:ident;)*) => {$(
            impl ContributionScope for $marker {
                #[track_caller]
                fn contribute<U, Args, F, C>(into: Contribute<'_, U>, factory: F, coerce: C)
                where
                    U: ?Sized + Send + Sync + 'static,
                    F: Factory<Args>,
                    F::Output: Send + Sync + 'static,
                    C: Fn(Arc<F::Output>) -> Arc<U> + Send + Sync + 'static,
                {
                    into.$plain::<Args, F>(factory, coerce);
                }

                #[track_caller]
                fn try_contribute<U, Args, F, T, E, C>(into: Contribute<'_, U>, factory: F, coerce: C)
                where
                    U: ?Sized + Send + Sync + 'static,
                    F: Factory<Args, Output = Result<T, E>>,
                    T: Send + Sync + 'static,
                    E: Into<BoxError> + Send + 'static,
                    C: Fn(Arc<T>) -> Arc<U> + Send + Sync + 'static,
                {
                    into.$fallible::<Args, F, T, E>(factory, coerce);
                }
            }
        )*};
    }

    contribution_scope! {
        Auto => with, try_with;
        Singleton => singleton, try_singleton;
        PerExecution => execution, try_execution;
        Transient => transient, try_transient;
    }
}

/// Runs one synchronous deserialisation with the secret collector open on this thread, answering
/// its value and every secret it produced (integrations §2.3, P6). `ulo-config` installs it around
/// each deserialisation it runs; `Secret<T>: Deserialize`, behind the `serde` feature, writes into
/// it, under `#[serde(flatten)]` and `#[serde(untagged)]` too. A collection open around this one
/// receives nothing of it. Without the `serde` feature nothing writes, and the answer is empty.
pub fn collect_secrets<R>(f: impl FnOnce() -> R) -> (R, CollectedSecrets) {
    let (answer, texts) = crate::redact::collecting(f);
    (answer, CollectedSecrets { texts })
}

/// The secrets one [`collect_secrets`] call recorded: the text of each scalar a `Secret`'s inner
/// value was read from, whatever its type, so a `Secret<T>` whose `T` is not `Display` is
/// recorded by the text it was read from. A value read more than once is recorded once per read;
/// the registry skips a repeat. `Debug` writes the count alone.
#[derive(Clone, Default)]
pub struct CollectedSecrets {
    texts: Vec<String>,
}

impl CollectedSecrets {
    /// Registers each secret with the module through `m.secret(..)`, so the graph's redaction
    /// replaces it in every error the graph reports.
    pub fn register(self, m: &mut crate::ModuleDef<'_>) {
        for text in self.texts {
            m.secret(&crate::Secret::new(text));
        }
    }

    /// Appends `other`'s secrets, for a caller that runs several deserialisations and registers
    /// them together.
    pub fn append(&mut self, mut other: CollectedSecrets) {
        self.texts.append(&mut other.texts);
    }

    /// The recorded texts, for a caller that keeps them beside other secrets it registers later.
    pub fn texts(&self) -> &[String] {
        &self.texts
    }

    pub fn len(&self) -> usize {
        self.texts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.texts.is_empty()
    }
}

impl std::fmt::Debug for CollectedSecrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "CollectedSecrets({} recorded)", self.texts.len())
    }
}
