//! A transport fixture for the enhancer pipeline tests (`CORE_TEST_PLAN.md` B5): two transports
//! whose context carries the call's execution and a [`Log`], the enhancers every test composes,
//! and the helpers that dispatch one call through a mounted handler.
//!
//! Nothing here is HTTP. A call is `dispatch` over the handlers `AppHandle::mounted` hands out, on
//! a connected app with a runtime, exactly as `scopes.rs` calls a controller.
//!
//! - [`TP`] and [`TQ`]: transports with `Cx = PCx` and `Reply = String`. Two, so a role key can be
//!   told apart per transport.
//! - [`G`], [`I`], [`E`]: a guard, an interceptor and an error handler for both transports, each
//!   with a name it writes to the log and a [`Verdict`], a panic switch or an [`Act`].
//! - `typed_guard!`, `typed_interceptor!`, `typed_error_handler!`, `typed_controller!`:
//!   `Construct` types of a scope of the test's choosing that read `Dep<Log>`, log `build <Name>`
//!   when the container builds them and `guard <Name>` or the like when they run.
//! - [`call`], [`call_in`], [`open`]: one dispatch, as the transport would run it.
//!
//! Every log line is `build <name>` (a construction), `guard <name>`, `enter <name>` and
//! `exit <name>`, `handle <name> [<kind>]`, or `handler`. [`kind_of`] names an error.

#![allow(dead_code, unused_macros)]

use std::future::{Ready, ready};
use std::sync::atomic::{AtomicUsize, Ordering};

use ulo::{
    App, BoxError, Connected, DispatchStage, ErrorHandler, ExecOptions, Execution, ExecutionRef, GuardRejected, Guard,
    Interceptor, LookupError, MountedHandler, Next, PanicRecovered, Transport,
};

use super::{Boom, Log};

/// The context of both fixture transports: the call's execution and the test's log.
#[derive(Clone)]
pub struct PCx {
    pub exec: ExecutionRef,
    pub log: Log,
}

pub struct TP;

impl Transport for TP {
    const KEY: &'static str = "p";
    type Cx = PCx;
    type Reply = String;
}

pub struct TQ;

impl Transport for TQ {
    const KEY: &'static str = "q";
    type Cx = PCx;
    type Reply = String;
}

/// The extension every [`call`] writes, as a guard would, for a closure that reads `Ext<User>`.
pub struct User;

/// What a [`G`] answers.
#[derive(Clone, Copy)]
pub enum Verdict {
    Admit,
    Refuse,
    Fail,
    Panic,
}

/// A guard for `TP` and `TQ`.
pub struct G {
    pub name: &'static str,
    pub verdict: Verdict,
}

impl G {
    pub fn new(name: &'static str) -> Self {
        G { name, verdict: Verdict::Admit }
    }

    pub fn refusing(name: &'static str) -> Self {
        G { name, verdict: Verdict::Refuse }
    }

    pub fn failing(name: &'static str) -> Self {
        G { name, verdict: Verdict::Fail }
    }

    pub fn panicking(name: &'static str) -> Self {
        G { name, verdict: Verdict::Panic }
    }

    async fn run(&self, cx: &PCx) -> Result<bool, BoxError> {
        cx.log.push(format!("guard {}", self.name));
        match self.verdict {
            Verdict::Admit => Ok(true),
            Verdict::Refuse => Ok(false),
            Verdict::Fail => Err(Box::new(Boom::new("guard failed"))),
            Verdict::Panic => panic!("guard {} panics", self.name),
        }
    }
}

/// An interceptor for `TP` and `TQ`: writes `enter` and, once the rest of the chain answered,
/// `exit <name> [<kind>]`, or panics before calling the rest.
pub struct I {
    pub name: &'static str,
    pub panics: bool,
}

impl I {
    pub fn new(name: &'static str) -> Self {
        I { name, panics: false }
    }

    pub fn panicking(name: &'static str) -> Self {
        I { name, panics: true }
    }
}

/// What an [`E`] does with the error it is offered.
#[derive(Clone, Copy)]
pub enum Act {
    /// Hands the same error on.
    Pass,
    /// Claims it with `claimed by <name>`.
    Claim,
    /// Hands a [`Reshaped`] on instead.
    Reshape,
    Panic,
}

/// The error an [`E`] doing [`Act::Reshape`] hands on.
#[derive(Debug)]
pub struct Reshaped(pub &'static str);

impl std::fmt::Display for Reshaped {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "reshaped by {}", self.0)
    }
}

impl std::error::Error for Reshaped {}

/// An error handler for `TP` and `TQ`: writes `handle <name> [<kind of the error>]`.
pub struct E {
    pub name: &'static str,
    pub act: Act,
}

impl E {
    pub fn new(name: &'static str, act: Act) -> Self {
        E { name, act }
    }

    pub fn pass(name: &'static str) -> Self {
        E::new(name, Act::Pass)
    }
}

/// How the log names an error: the pipeline's own errors by type and field, a [`Boom`] and a
/// [`Reshaped`] by type.
pub fn kind_of(err: &BoxError) -> String {
    if let Some(rejected) = err.downcast_ref::<GuardRejected>() {
        let _ = rejected;
        "rejected".to_owned()
    } else if let Some(panic) = err.downcast_ref::<PanicRecovered>() {
        format!("panic:{:?}", panic.stage)
    } else if err.is::<Boom>() {
        "boom".to_owned()
    } else if err.is::<Reshaped>() {
        "reshaped".to_owned()
    } else if err.is::<LookupError>() {
        "lookup".to_owned()
    } else {
        "other".to_owned()
    }
}

async fn intercept_run(name: &'static str, panics: bool, cx: &PCx, next: Next<'_, impl Transport<Cx = PCx, Reply = String>>) -> Result<String, BoxError> {
    cx.log.push(format!("enter {name}"));
    if panics {
        panic!("interceptor {name} panics");
    }
    let answer = next.run().await;
    let kind = match &answer {
        Ok(_) => "ok".to_owned(),
        Err(err) => kind_of(err),
    };
    cx.log.push(format!("exit {name} [{kind}]"));
    answer
}

async fn handle_run(name: &'static str, act: Act, err: BoxError, cx: &PCx) -> Result<String, BoxError> {
    cx.log.push(format!("handle {name} [{}]", kind_of(&err)));
    match act {
        Act::Pass => Err(err),
        Act::Claim => Ok(format!("claimed by {name}")),
        Act::Reshape => Err(Box::new(Reshaped(name))),
        Act::Panic => panic!("error handler {name} panics"),
    }
}

macro_rules! roles_for {
    ($($t:ty),+) => {$(
        impl Guard<$t> for G {
            async fn can_activate(&self, cx: &PCx) -> Result<bool, BoxError> {
                self.run(cx).await
            }
        }

        impl Interceptor<$t> for I {
            async fn intercept(&self, cx: &PCx, next: Next<'_, $t>) -> Result<String, BoxError> {
                intercept_run(self.name, self.panics, cx, next).await
            }
        }

        impl ErrorHandler<$t> for E {
            async fn handle(&self, err: BoxError, cx: &PCx) -> Result<String, BoxError> {
                handle_run(self.name, self.act, err, cx).await
            }
        }
    )+};
}

roles_for!(TP, TQ);

/// A closure body for `guard_with` and the like: logs `build <name>` through the injected log and
/// answers a guard.
pub fn built_guard(log: &super::Log, name: &'static str) -> Ready<G> {
    log.push(format!("build {name}"));
    ready(G::new(name))
}

pub fn built_interceptor(log: &super::Log, name: &'static str) -> Ready<I> {
    log.push(format!("build {name}"));
    ready(I::new(name))
}

pub fn built_handler(log: &super::Log, name: &'static str, act: Act) -> Ready<E> {
    log.push(format!("build {name}"));
    ready(E::new(name, act))
}

/// A counter a guard bumps each time it runs, for a test that asks how many calls one instance
/// served. Logs `serve <name> <n>`.
pub struct Counting {
    pub name: &'static str,
    pub served: AtomicUsize,
}

impl Counting {
    pub fn new(name: &'static str) -> Self {
        Counting { name, served: AtomicUsize::new(0) }
    }
}

impl Guard<TP> for Counting {
    async fn can_activate(&self, cx: &PCx) -> Result<bool, BoxError> {
        let n = self.served.fetch_add(1, Ordering::SeqCst) + 1;
        cx.log.push(format!("serve {} {n}", self.name));
        Ok(true)
    }
}

/// What the handler body of a [`call`] does.
#[derive(Clone, Copy)]
pub enum Body {
    Ok,
    Fail,
    Panic,
}

/// The mounted handlers of `T`, in mount order.
pub fn mounted<T: Transport>(app: &App<Connected>) -> Vec<MountedHandler<T>> {
    app.handle().mounted::<T>().expect("the app has a runtime")
}

/// The one mounted handler of `T`.
pub fn only<T: Transport>(app: &App<Connected>) -> MountedHandler<T> {
    let mut handlers = mounted::<T>(app);
    assert_eq!(handlers.len(), 1, "one handler was mounted");
    handlers.remove(0)
}

/// The mounted handler named `name`.
pub fn named<T: Transport>(app: &App<Connected>, name: &str) -> MountedHandler<T> {
    mounted::<T>(app)
        .into_iter()
        .find(|handler| handler.name() == name)
        .unwrap_or_else(|| panic!("no handler named `{name}` was mounted"))
}

/// An execution in the handler's module with the [`User`] extension written.
pub fn open<T: Transport>(handler: &MountedHandler<T>) -> Execution {
    let exec = Execution::open(handler.module(), ExecOptions::new()).expect("the app is running");
    exec.extensions().insert(User);
    exec
}

/// One dispatch inside `exec`. A second call on one execution shares its per-execution instances.
pub async fn call_in<T>(handler: &MountedHandler<T>, exec: &Execution, log: &Log, body: Body) -> Result<String, BoxError>
where
    T: Transport<Cx = PCx, Reply = String>,
{
    let calls = exec.handle();
    let cx = PCx { exec: calls.clone(), log: log.clone() };
    ulo::dispatch(handler, &calls, &cx, move |cx: PCx| async move {
        cx.log.push("handler");
        match body {
            Body::Ok => Ok("handled".to_owned()),
            Body::Fail => Err(Box::new(Boom::new("handler failed")) as BoxError),
            Body::Panic => panic!("the handler panics"),
        }
    })
    .await
}

/// One call, in an execution of its own.
pub async fn call<T>(handler: &MountedHandler<T>, log: &Log, body: Body) -> Result<String, BoxError>
where
    T: Transport<Cx = PCx, Reply = String>,
{
    let exec = open(handler);
    call_in(handler, &exec, log, body).await
}

/// The stage a [`PanicRecovered`] error names.
pub fn stage_of(err: &BoxError) -> Option<DispatchStage> {
    err.downcast_ref::<PanicRecovered>().map(|panic| panic.stage)
}

/// Binds the test's [`Log`] so `Dep<Log>` reads it.
pub fn bind_log(m: &mut ulo::ModuleDef<'_>, log: &Log) {
    m.value(log.clone());
}

/// A `Construct` guard of scope `$scope` named `$name`: logs `build $name` when built through
/// `Dep<Log>` and `guard $name` when it runs, admitting. `reads $ty` adds an injection point.
macro_rules! typed_guard {
    ($name:ident, $scope:ty $(, reads $ty:ty)?) => {
        pub struct $name;

        impl ulo::Construct for $name {
            type Scope = $scope;

            fn dependencies(d: &mut ulo::Dependencies) {
                d.field::<ulo::Dep<$crate::common::Log>>("log");
                $( d.field::<$ty>("read"); )?
            }

            async fn construct(r: &ulo::Resolver<'_>) -> Result<Self, ulo::ConstructError> {
                let log = <ulo::Dep<$crate::common::Log> as ulo::FromContainer>::from_container(r).await?;
                $( let _ = <$ty as ulo::FromContainer>::from_container(r).await?; )?
                log.push(concat!("build ", stringify!($name)));
                Ok($name)
            }
        }

        impl ulo::Guard<$crate::common::pipeline::TP> for $name {
            async fn can_activate(&self, cx: &$crate::common::pipeline::PCx) -> Result<bool, ulo::BoxError> {
                cx.log.push(concat!("guard ", stringify!($name)));
                Ok(true)
            }
        }
    };
}

macro_rules! typed_interceptor {
    ($name:ident, $scope:ty $(, reads $ty:ty)?) => {
        pub struct $name;

        impl ulo::Construct for $name {
            type Scope = $scope;

            fn dependencies(d: &mut ulo::Dependencies) {
                d.field::<ulo::Dep<$crate::common::Log>>("log");
                $( d.field::<$ty>("read"); )?
            }

            async fn construct(r: &ulo::Resolver<'_>) -> Result<Self, ulo::ConstructError> {
                let log = <ulo::Dep<$crate::common::Log> as ulo::FromContainer>::from_container(r).await?;
                $( let _ = <$ty as ulo::FromContainer>::from_container(r).await?; )?
                log.push(concat!("build ", stringify!($name)));
                Ok($name)
            }
        }

        impl ulo::Interceptor<$crate::common::pipeline::TP> for $name {
            async fn intercept(
                &self,
                cx: &$crate::common::pipeline::PCx,
                next: ulo::Next<'_, $crate::common::pipeline::TP>,
            ) -> Result<String, ulo::BoxError> {
                cx.log.push(concat!("enter ", stringify!($name)));
                let answer = next.run().await;
                cx.log.push(concat!("exit ", stringify!($name)));
                answer
            }
        }
    };
}

/// A `Construct` error handler that passes every error on after logging `handle $name [kind]`.
macro_rules! typed_error_handler {
    ($name:ident, $scope:ty $(, reads $ty:ty)?) => {
        pub struct $name;

        impl ulo::Construct for $name {
            type Scope = $scope;

            fn dependencies(d: &mut ulo::Dependencies) {
                d.field::<ulo::Dep<$crate::common::Log>>("log");
                $( d.field::<$ty>("read"); )?
            }

            async fn construct(r: &ulo::Resolver<'_>) -> Result<Self, ulo::ConstructError> {
                let log = <ulo::Dep<$crate::common::Log> as ulo::FromContainer>::from_container(r).await?;
                $( let _ = <$ty as ulo::FromContainer>::from_container(r).await?; )?
                log.push(concat!("build ", stringify!($name)));
                Ok($name)
            }
        }

        impl ulo::ErrorHandler<$crate::common::pipeline::TP> for $name {
            async fn handle(
                &self,
                err: ulo::BoxError,
                cx: &$crate::common::pipeline::PCx,
            ) -> Result<String, ulo::BoxError> {
                cx.log.push(format!("handle {} [{}]", stringify!($name), $crate::common::pipeline::kind_of(&err)));
                Err(err)
            }
        }
    };
}

/// A controller of scope `$scope` that logs `build $name` and mounts what `|m| body` declares.
macro_rules! typed_controller {
    ($name:ident, $scope:ty, |$m:ident| $body:expr) => {
        pub struct $name;

        impl ulo::Construct for $name {
            type Scope = $scope;

            fn dependencies(d: &mut ulo::Dependencies) {
                d.field::<ulo::Dep<$crate::common::Log>>("log");
            }

            async fn construct(r: &ulo::Resolver<'_>) -> Result<Self, ulo::ConstructError> {
                let log = <ulo::Dep<$crate::common::Log> as ulo::FromContainer>::from_container(r).await?;
                log.push(concat!("build ", stringify!($name)));
                Ok($name)
            }
        }

        impl ulo::Controller for $name {
            fn mount($m: &mut ulo::Mount<'_>) {
                $body
            }
        }
    };
}
