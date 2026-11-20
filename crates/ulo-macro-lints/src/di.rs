//! `#[injectable]` on a struct and on an impl, with each scope, a timeout, `#[construct]`, an
//! `async` fallible constructor, a lifecycle hook, and constructor parameters and fields behind
//! a `#[cfg]` that is off and one that is on.

use std::time::Duration;

use ulo::{BoxError, Dep, OnModuleInit, Runtime, Timer, injectable};

/// A struct-form injectable with no dependency.
#[injectable(singleton)]
pub struct Clock;

/// A struct-form injectable with a dependency and a defaulted field.
#[injectable]
pub struct Store {
    pub clock: Dep<Clock>,
    #[injectable(default)]
    pub hits: u64,
}

/// An impl-form injectable whose constructor is `new`, `async` and fallible, with a timeout and a
/// lifecycle hook.
pub struct Named {
    pub name: String,
}

#[injectable(execution, timeout = Duration::from_secs(1))]
impl Named {
    pub async fn new(clock: Dep<Clock>, timer: Dep<dyn Timer>) -> Result<Self, BoxError> {
        timer.sleep(Duration::ZERO).await;
        let _ = clock;
        Ok(Named { name: "named".to_owned() })
    }
}

/// An impl-form injectable whose constructor is marked `#[construct]`.
pub struct Defaulted {
    pub store: Dep<Store>,
}

#[injectable(transient)]
impl Defaulted {
    #[construct]
    pub fn build(store: Dep<Store>, runtime: Dep<dyn Runtime>) -> Self {
        let _ = runtime;
        Defaulted { store }
    }
}

/// An impl-form injectable whose constructor has a parameter behind a `#[cfg]` that is off and one
/// behind a `#[cfg]` that is on.
pub struct GatedCtor {
    pub clock: Dep<Clock>,
}

#[injectable]
impl GatedCtor {
    pub fn new(#[cfg(any())] absent: Dep<Clock>, #[cfg(not(any()))] clock: Dep<Clock>) -> Self {
        GatedCtor { clock }
    }
}

/// A struct-form injectable with fields behind a `#[cfg]` that is off and one that is on, injected
/// and defaulted.
#[injectable]
pub struct GatedFields {
    #[cfg(any())]
    pub absent: Dep<Clock>,
    #[cfg(not(any()))]
    pub clock: Dep<Clock>,
    #[cfg(any())]
    #[injectable(default)]
    pub absent_hits: u64,
    #[cfg(not(any()))]
    #[injectable(default)]
    pub hits: u64,
}

/// A tuple injectable whose gated field renumbers the fields after it.
#[injectable]
pub struct GatedTuple(#[cfg(any())] pub Dep<Clock>, pub Dep<Clock>, #[cfg(not(any()))] #[injectable(default)] pub u64);

impl OnModuleInit for Clock {
    async fn on_module_init(&self) -> Result<(), BoxError> {
        Ok(())
    }
}

/// What a braced constant in a `providers` list binds.
#[derive(Clone, Copy)]
pub struct Limits {
    pub max: u32,
}

pub const LIMITS: Limits = Limits { max: 8 };

/// The qualifier `Limits` is bound under a second time.
pub struct Strict;

/// The qualifier of the fallible braced entry.
pub struct Loaded;

/// What a braced fallible entry, `{ loaded_limits() }?`, binds.
pub fn loaded_limits() -> Result<Limits, BoxError> {
    Ok(LIMITS)
}

/// A braced constant, `{ LIMITS }`, with and without `@ Q`, and a braced fallible value,
/// `{ .. }? @ Q`. The braces tell a value from a type and are not the lowering's to repeat:
/// written again as the argument of `m.value(..)` or `m.try_value(..)` they trip `unused_braces`
/// in the user's crate.
#[ulo::module(providers = [{ LIMITS }, { LIMITS } @ Strict, { loaded_limits() }? @ Loaded])]
pub struct BracedValues;
