//! Shared harness for the core test series (`CORE_TEST_PLAN.md` section 0).
//!
//! Every integration test file declares `#[macro_use] mod common;` and uses the value-level API
//! only; no macro from `ulo-macros` appears here. Not every file uses every helper, so unused
//! items are expected and allowed.
//!
//! # What is here
//!
//! - [`Empty`]: a module that registers nothing.
//! - [`Mod<K>`]: a module of a type of its own for each marker `K`, built from a closure. Two
//!   `Mod<A>` and `Mod<B>` are two modules; `Mod<A>` imported twice is one. A module prints as
//!   `Mod<A>`, with `A` cut to its last path segment; [`mod_name`] writes that text for assertions.
//! - [`wire_err`], [`wire_err_with`], [`wiring_errors`]: make `wire()` fail and hand back the
//!   [`WiringErrors`]; they panic with the real outcome when `wire()` succeeds or fails another way.
//! - [`wires`]: the opposite, `wire()` must succeed.
//! - [`entry!`] and [`entries!`]: pick the entries of one variant out of a report and destructure
//!   them in one expression, so an assertion names variant and fields and not message text.
//! - [`variants`]: the variant name of each entry, in report order (steps 1 to 6 of section 10.1).
//! - [`Probe`]: records the order in which fixtures were constructed or hooks ran.
//! - [`Frozen`]: a `Timer` whose every sleep ends at once, so an app has a clock without a runtime.
//! - [`TA`], [`TB`], [`GuardA`]: two test transports and a guard for one of them, for role keys.
//! - [`unit_service!`]: unit structs the container can build, for `provide::<S>()`.
//! - [`watchdog`]: aborts the test process if it runs past 60 s, so a hang is a failure and never a stall.
//!   Every wiring helper starts it.
//! - [`Boom`]: an error type with a message, for failing factories and `try_value`.
//!
//! Tests that need a runtime (hooks, constructor waits) use `#[tokio::test]` with
//! `ulo_runtime_tokio::Tokio::current()`; B1 needs none, since wiring is synchronous.

// `StartupError` is large by design; the helpers return it as `wire()` does.
#![allow(dead_code, unused_macros, clippy::result_large_err)]

use std::error::Error;
use std::fmt;
use std::marker::PhantomData;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use ulo::{
    App, AppBuilder, BoxError, BoxFuture, Guard, Module, ModuleDef, ModuleIdentity, StartupError, Timer, Transport, TypeName,
    Wired, WiringError, WiringErrors,
};

/// A module that registers nothing.
pub struct Empty;

impl Module for Empty {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, _m: &mut ModuleDef<'_>) {}
}

/// A module whose registration is a closure and whose type is `Mod<K>`. Declare a marker per
/// module (`struct Billing;`) and write `Mod::<Billing>::new(|m| { .. })`; the marker is the
/// module's identity and its printed name.
pub struct Mod<K: 'static> {
    register: Box<dyn Fn(&mut ModuleDef<'_>) + Send + Sync>,
    _k: PhantomData<fn() -> K>,
}

impl<K: 'static> Mod<K> {
    pub fn new(register: impl Fn(&mut ModuleDef<'_>) + Send + Sync + 'static) -> Self {
        Mod { register: Box::new(register), _k: PhantomData }
    }

    /// `Mod<K>` registering nothing.
    pub fn bare() -> Self {
        Mod::new(|_| {})
    }
}

impl<K: 'static> Module for Mod<K> {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        (self.register)(m);
    }
}

/// How a report prints `Mod<K>`: `Mod<K>` with `K` cut to its last path segment.
pub fn mod_name<K: 'static>() -> String {
    format!("Mod<{}>", TypeName::of::<K>())
}

/// How a keyed `Mod<K>` prints: `Mod<K> @ Q`.
pub fn keyed_mod_name<K: 'static, Q: 'static>() -> String {
    format!("{} @ {}", mod_name::<K>(), TypeName::of::<Q>())
}

/// Starts, once per test process, a thread that aborts the process 60 seconds in. The suite takes
/// well under a second, so reaching the deadline means a test hung.
pub fn watchdog() {
    static STARTED: std::sync::Once = std::sync::Once::new();
    STARTED.call_once(|| {
        std::thread::spawn(|| {
            std::thread::sleep(Duration::from_secs(60));
            eprintln!("watchdog: the test process ran past 60 s; a test is hung");
            std::process::abort();
        });
    });
}

/// `wire()` on `root` with the default builder, which has no `Timer`.
pub fn wire_result(root: impl Module) -> Result<App<Wired>, StartupError> {
    watchdog();
    App::builder(root).wire()
}

/// The report of a `wire()` that must fail with wiring errors.
pub fn wire_err(root: impl Module) -> WiringErrors {
    wiring_errors(wire_result(root))
}

/// As [`wire_err`], for a builder the test configured (a timer, a knob).
pub fn wire_err_with(builder: AppBuilder) -> WiringErrors {
    watchdog();
    wiring_errors(builder.wire())
}

/// The report inside a `wire()` outcome. Panics, saying what happened instead, on success and on a
/// startup error that is not `Wiring`.
pub fn wiring_errors(outcome: Result<App<Wired>, StartupError>) -> WiringErrors {
    match outcome {
        Err(StartupError::Wiring(errors)) => errors,
        Err(other) => panic!("wire() failed, but not with wiring errors: {other}"),
        Ok(_) => panic!("wire() succeeded; the graph was expected to be refused"),
    }
}

/// The wired app of a `wire()` that must succeed; the report is the panic message otherwise.
pub fn wires(root: impl Module) -> App<Wired> {
    wires_with(App::builder(root))
}

pub fn wires_with(builder: AppBuilder) -> App<Wired> {
    watchdog();
    match builder.wire() {
        Ok(app) => app,
        Err(error) => panic!("wire() refused a graph that should wire:\n{error}"),
    }
}

/// The variant name of every entry, in report order. The report lists entries by step, so this is
/// also the order of section 10.1.
pub fn variants(errors: &WiringErrors) -> Vec<String> {
    errors.iter().map(variant).collect()
}

/// `Missing` for `WiringError::Missing { .. }`: the derived `Debug` opens with the variant name.
pub fn variant(error: &WiringError) -> String {
    format!("{error:?}").split([' ', '{', '(']).next().unwrap_or_default().to_owned()
}

/// Every entry of `$errors` matching `$pat`, as a `Vec` of `$out`. `$pat` binds fields by
/// reference.
macro_rules! entries {
    ($errors:expr, $pat:pat => $out:expr) => {{
        let errors: &ulo::WiringErrors = &$errors;
        errors
            .iter()
            .filter_map(|entry| match entry {
                $pat => Some($out),
                _ => None,
            })
            .collect::<Vec<_>>()
    }};
}

/// The one entry of `$errors` matching `$pat`, as `$out`. Fails the test, printing the whole
/// report, when no entry or several match.
macro_rules! entry {
    ($errors:expr, $pat:pat => $out:expr) => {{
        let mut found = entries!($errors, $pat => $out);
        assert!(
            found.len() == 1,
            "expected exactly one `{}` entry, found {}; the report:\n{}",
            stringify!($pat),
            found.len(),
            $errors
        );
        found.remove(0)
    }};
}

/// An error with a message, `Send + Sync` and `std::error::Error`.
#[derive(Debug)]
pub struct Boom(pub String);

impl Boom {
    pub fn new(text: impl Into<String>) -> Self {
        Boom(text.into())
    }
}

impl fmt::Display for Boom {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Error for Boom {}

/// Records the order in which fixtures were constructed or hooks ran.
#[derive(Clone, Default)]
pub struct Probe(Arc<Mutex<Vec<&'static str>>>);

impl Probe {
    pub fn new() -> Self {
        Probe::default()
    }

    pub fn mark(&self, what: &'static str) {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).push(what);
    }

    pub fn log(&self) -> Vec<&'static str> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    pub fn is_empty(&self) -> bool {
        self.log().is_empty()
    }
}

/// A clock that stands still a day ahead and ends every sleep at once. It gives an app a `Timer`
/// without a runtime.
pub struct Frozen(Instant);

impl Frozen {
    pub fn new() -> Self {
        Frozen(Instant::now() + Duration::from_secs(86_400))
    }
}

impl Timer for Frozen {
    fn sleep(&self, _d: Duration) -> BoxFuture<'static, ()> {
        Box::pin(async {})
    }

    fn now(&self) -> Instant {
        self.0
    }
}

/// Unit structs the container can build, so a test can `m.provide::<S>()` without a macro.
macro_rules! unit_service {
    ($($name:ident),+ $(,)?) => {$(
        pub struct $name;

        impl ulo::Construct for $name {
            type Scope = ulo::scope::Singleton;

            fn dependencies(_d: &mut ulo::Dependencies) {}

            async fn construct(_r: &ulo::Resolver<'_>) -> Result<Self, ulo::ConstructError> {
                Ok($name)
            }
        }
    )+};
}

/// Two transports with unit contexts; their role keys are `AnyGuard<TA>` and `AnyGuard<TB>`.
pub struct TA;

impl Transport for TA {
    const KEY: &'static str = "a";
    type Cx = ();
    type Reply = ();
}

pub struct TB;

impl Transport for TB {
    const KEY: &'static str = "b";
    type Cx = ();
    type Reply = ();
}

/// A guard for `TA` that admits everything.
pub struct GuardA;

impl Guard<TA> for GuardA {
    async fn can_activate(&self, _cx: &()) -> Result<bool, BoxError> {
        Ok(true)
    }
}

impl ulo::Construct for GuardA {
    type Scope = ulo::scope::Singleton;

    fn dependencies(_d: &mut ulo::Dependencies) {}

    async fn construct(_r: &ulo::Resolver<'_>) -> Result<Self, ulo::ConstructError> {
        Ok(GuardA)
    }
}
