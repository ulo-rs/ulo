//! The enhancer pipeline in the core (DESIGN section 7), run through `dispatch` over mounted
//! handlers of the fixture transports in `common::pipeline`.
//!
//! Capabilities pinned: 24 (an enhancer declared by type takes the scope its type declares, a
//! singleton or one per execution, and reads what the container holds), 26 (declared by type, by
//! value and by closure; the stack order global, controller, method in written order; a closure's
//! scope follows the `Auto` rule and an explicit scope overrides it), 27 (each guard is built only
//! once the one before it admits, nothing below the guards before all admit), and the run-time
//! rows of section 3 of the plan: a guard's refusal reaches the error handlers as `GuardRejected`,
//! a panic is caught at its stage as `PanicRecovered` (a panicking closure counts as the stage of the
//! enhancer it builds), and `is_panic` reads both shapes.
//! Error handlers run method, controller, global, each tier last declared first, and the first
//! `Ok` claims (DESIGN 7, step 4). A controller-level value is built once and shared by every
//! handler of the controller (transports DESIGN X2).
//!
//! The role-key rows (25, 28, 29) are in `enhancer_roles.rs`.

#[macro_use]
mod common;

use std::any::type_name;
use std::future::ready;
use std::sync::Arc;

use common::pipeline::*;
use common::*;
use ulo::scope::{Auto, PerExecution, Singleton, Transient};
use ulo::{
    AnyErrorHandler, AnyGuard, AnyInterceptor, BoxError, Dep, EnhancerSpec, Ext, FailureReason, GuardRejected,
    HandlerSpec, Key, LookupError, PanicRecovered, is_panic,
};

struct A;

/// What the log wrote after `verb`, for every line that begins with it, in order.
fn after(log: &Log, verb: &str) -> Vec<String> {
    let prefix = format!("{verb} ");
    log.events().into_iter().filter_map(|line| line.strip_prefix(&prefix).map(str::to_owned)).collect()
}

/// The lines written after the first `from`, for a test whose controller logged its own build at
/// connect.
fn since(log: &Log, from: usize) -> Vec<String> {
    log.events().split_off(from)
}

fn strs(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_owned()).collect()
}

typed_guard!(TypedC2, Singleton);
typed_guard!(TypedM3, Singleton);
typed_interceptor!(TypedIc2, Singleton);
typed_error_handler!(TypedEc2, Singleton);

typed_controller!(Ordered, Singleton, |m| {
    let mut controller = EnhancerSpec::<TP>::new();
    controller
        .guard_with(|log: Dep<Log>| built_guard(&log, "c1"))
        .guard::<TypedC2>()
        .guard_value(G::new("c3"))
        .interceptor_with(|log: Dep<Log>| built_interceptor(&log, "ic1"))
        .interceptor::<TypedIc2>();
    let mut method = EnhancerSpec::<TP>::new();
    method
        .guard_value(G::new("m1"))
        .guard_with(|log: Dep<Log>| built_guard(&log, "m2"))
        .guard::<TypedM3>()
        .interceptor_value(I::new("im1"))
        .interceptor_with(|log: Dep<Log>| built_interceptor(&log, "im2"));
    m.handler::<TP, _>(HandlerSpec::new("h", ()).controller(controller).method(method));
});

fn ordered(log: &Log) -> impl ulo::Module {
    let log = log.clone();
    Mod::<A>::new(move |m| {
        bind_log(m, &log);
        m.provide::<TypedC2>();
        m.provide::<TypedM3>();
        m.provide::<TypedIc2>();
        m.enhancer::<AnyGuard<TP>>().value(Arc::new(G::new("g1")));
        m.enhancer::<AnyGuard<TP>>().singleton(|log: Dep<Log>| built_guard(&log, "g2"), |a| a);
        m.enhancer::<AnyInterceptor<TP>>().value(Arc::new(I::new("ig1")));
        m.enhancer::<AnyInterceptor<TP>>().value(Arc::new(I::new("ig2")));
        m.controller::<Ordered>();
    })
}

#[tokio::test]
async fn the_three_spellings_stack_global_controller_method_in_written_order() {
    let log = Log::new();
    let app = connected_on_tokio(ordered(&log)).await;
    let handler = only::<TP>(&app);
    let reply = within("the call", call(&handler, &log, Body::Ok)).await.expect("every guard admits");
    assert_eq!(reply, "handled");
    // Global in collection order; within a tier, a closure, a type and a value interleave as
    // written, whichever spelling each entry uses.
    assert_eq!(
        after(&log, "guard"),
        strs(&["g1", "g2", "c1", "TypedC2", "c3", "m1", "m2", "TypedM3"]),
        "{:?}",
        log.events()
    );
    drop(handler);
    close(app).await;
}

#[tokio::test]
async fn interceptors_nest_global_outermost_then_controller_then_method_around_the_handler() {
    let log = Log::new();
    let app = connected_on_tokio(ordered(&log)).await;
    let handler = only::<TP>(&app);
    within("the call", call(&handler, &log, Body::Ok)).await.expect("the call answers");
    let wrapped: Vec<String> = log
        .events()
        .into_iter()
        .filter(|line| line.starts_with("enter ") || line.starts_with("exit ") || line == "handler")
        .collect();
    // Each tier keeps the order written, whichever spelling each entry uses, and the
    // interceptors that wrap the handler first see it last.
    assert_eq!(
        wrapped,
        strs(&[
            "enter ig1",
            "enter ig2",
            "enter ic1",
            "enter TypedIc2",
            "enter im1",
            "enter im2",
            "handler",
            "exit im2 [ok]",
            "exit im1 [ok]",
            "exit TypedIc2",
            "exit ic1 [ok]",
            "exit ig2 [ok]",
            "exit ig1 [ok]",
        ]),
        "{:?}",
        log.events()
    );
    drop(handler);
    close(app).await;
}

typed_controller!(Lazy, PerExecution, |m| {
    let mut refused = EnhancerSpec::<TP>::new();
    refused
        .guard_with(|log: Dep<Log>, _u: Ext<User>| {
            log.push("build r1");
            ready(G::refusing("r1"))
        })
        .guard_with(|log: Dep<Log>, _u: Ext<User>| built_guard(&log, "r2"))
        .interceptor_with(|log: Dep<Log>, _u: Ext<User>| built_interceptor(&log, "ri"));
    m.handler::<TP, _>(HandlerSpec::new("refused", ()).method(refused));
    // Two per-execution closures in one execution, so each must be built into a slot of its own.
    let mut admitted = EnhancerSpec::<TP>::new();
    admitted
        .guard_with(|log: Dep<Log>, _u: Ext<User>| built_guard(&log, "a1"))
        .guard_with(|log: Dep<Log>, _u: Ext<User>| built_guard(&log, "a2"));
    m.handler::<TP, _>(HandlerSpec::new("admitted", ()).method(admitted));
});

fn lazy(log: &Log) -> impl ulo::Module {
    let log = log.clone();
    Mod::<A>::new(move |m| {
        bind_log(m, &log);
        m.enhancer::<AnyGuard<TP>>().execution(|log: Dep<Log>| built_guard(&log, "g1"), |a| a);
        m.enhancer::<AnyInterceptor<TP>>().execution(|log: Dep<Log>| built_interceptor(&log, "i1"), |a| a);
        m.controller::<Lazy>();
    })
}

#[tokio::test]
async fn guards_are_built_one_by_one_and_lower_stages_wait() {
    let log = Log::new();
    let app = connected_on_tokio(lazy(&log)).await;
    assert!(log.events().is_empty(), "nothing per execution is built at connect: {:?}", log.events());
    let handler = named::<TP>(&app, "refused");
    let err = within("the call", call(&handler, &log, Body::Ok)).await.expect_err("the second guard refuses");
    // g1 admits, r1 refuses: r2, the interceptors (global i1 and method ri), the controller and
    // the handler are never built or run.
    assert_eq!(log.events(), strs(&["build g1", "guard g1", "build r1", "guard r1"]));
    let rejected = err.downcast_ref::<GuardRejected>().expect("a refusal is a GuardRejected");
    assert_eq!(rejected.guard, type_name::<G>());
    drop(handler);
    close(app).await;
}

#[tokio::test]
async fn once_every_guard_admits_interceptors_are_built_then_the_controller_then_the_handler_runs() {
    let log = Log::new();
    let app = connected_on_tokio(lazy(&log)).await;
    let handler = named::<TP>(&app, "admitted");
    within("the call", call(&handler, &log, Body::Ok)).await.expect("every guard admits");
    assert_eq!(
        log.events(),
        strs(&[
            "build g1",
            "guard g1",
            "build a1",
            "guard a1",
            "build a2",
            "guard a2",
            "build i1",
            "build Lazy",
            "enter i1",
            "handler",
            "exit i1 [ok]",
        ])
    );
    drop(handler);
    close(app).await;
}

typed_controller!(Handled, Singleton, |m| {
    let mut controller = EnhancerSpec::<TP>::new();
    controller.error_handler_with(|log: Dep<Log>| built_handler(&log, "ec1", Act::Pass)).error_handler::<TypedEc2>();
    let mut method = EnhancerSpec::<TP>::new();
    method
        .error_handler_value(E::pass("em1"))
        .error_handler_with(|log: Dep<Log>| built_handler(&log, "em2", Act::Pass));
    m.handler::<TP, _>(HandlerSpec::new("h", ()).controller(controller).method(method));
});

#[tokio::test]
async fn error_handlers_run_method_then_controller_then_global_each_tier_last_declared_first() {
    let log = Log::new();
    let l = log.clone();
    let app = connected_on_tokio(Mod::<A>::new(move |m| {
        bind_log(m, &l);
        m.provide::<TypedEc2>();
        m.enhancer::<AnyErrorHandler<TP>>().value(Arc::new(E::pass("eg1")));
        m.enhancer::<AnyErrorHandler<TP>>().singleton(|log: Dep<Log>| built_handler(&log, "eg2", Act::Pass), |a| a);
        m.controller::<Handled>();
    }))
    .await;
    let handler = only::<TP>(&app);
    let err = within("the call", call(&handler, &log, Body::Fail)).await.expect_err("nothing claims the error");
    assert_eq!(
        after(&log, "handle"),
        strs(&[
            "em2 [boom]",
            "em1 [boom]",
            "TypedEc2 [boom]",
            "ec1 [boom]",
            "eg2 [boom]",
            "eg1 [boom]",
        ]),
        "{:?}",
        log.events()
    );
    assert!(err.is::<Boom>(), "an error nothing claims comes back as it was raised");
    drop(handler);
    close(app).await;
}

typed_controller!(Claiming, Singleton, |m| {
    let mut controller = EnhancerSpec::<TP>::new();
    controller.error_handler_value(E::new("ec1", Act::Claim));
    let mut method = EnhancerSpec::<TP>::new();
    method.error_handler_value(E::pass("em1")).error_handler_value(E::new("em2", Act::Reshape));
    m.handler::<TP, _>(HandlerSpec::new("h", ()).controller(controller).method(method));
});

#[tokio::test]
async fn the_first_claim_ends_the_walk_and_a_reshaped_error_is_what_the_next_handler_sees() {
    let log = Log::new();
    let l = log.clone();
    let app = connected_on_tokio(Mod::<A>::new(move |m| {
        bind_log(m, &l);
        m.enhancer::<AnyErrorHandler<TP>>().value(Arc::new(E::pass("eg1")));
        m.controller::<Claiming>();
    }))
    .await;
    let handler = only::<TP>(&app);
    let reply = within("the call", call(&handler, &log, Body::Fail)).await.expect("a handler claims the error");
    assert_eq!(reply, "claimed by ec1");
    // em2 reshapes the handler's error, em1 is offered the reshaped one and passes it on, ec1
    // claims it; the global handler is never offered anything.
    assert_eq!(after(&log, "handle"), strs(&["em2 [boom]", "em1 [reshaped]", "ec1 [reshaped]"]));
    drop(handler);
    close(app).await;
}

typed_controller!(Refusals, Singleton, |m| {
    let mut refuse = EnhancerSpec::<TP>::new();
    refuse.guard_value(Counting::new("c")).guard_value(G::refusing("r")).error_handler_value(E::pass("e"));
    m.handler::<TP, _>(HandlerSpec::new("refuse", ()).method(refuse));
    let mut fail = EnhancerSpec::<TP>::new();
    fail.guard_value(G::failing("f")).error_handler_value(E::pass("e"));
    m.handler::<TP, _>(HandlerSpec::new("fail", ()).method(fail));
});

#[tokio::test]
async fn a_guard_refusal_reaches_the_error_handlers_as_guard_rejected() {
    let log = Log::new();
    let l = log.clone();
    let app = connected_on_tokio(Mod::<A>::new(move |m| {
        bind_log(m, &l);
        m.controller::<Refusals>();
    }))
    .await;
    let built = log.events().len();
    let refuse = named::<TP>(&app, "refuse");
    let err = within("the call", call(&refuse, &log, Body::Ok)).await.expect_err("the guard refuses");
    assert_eq!(since(&log, built), strs(&["serve c 1", "guard r", "handle e [rejected]"]), "the handler never runs");
    // The guard that refused, not the one that admitted before it.
    assert_eq!(err.downcast_ref::<GuardRejected>().expect("GuardRejected").guard, type_name::<G>());

    // A guard that answers `Err` is that error, not a refusal.
    let before = log.events().len();
    let fail = named::<TP>(&app, "fail");
    let err = within("the call", call(&fail, &log, Body::Ok)).await.expect_err("the guard fails");
    assert_eq!(since(&log, before), strs(&["guard f", "handle e [boom]"]));
    assert!(err.downcast_ref::<GuardRejected>().is_none());
    drop((refuse, fail));
    close(app).await;
}

struct PanicsOnBuild;

impl ulo::Construct for PanicsOnBuild {
    type Scope = PerExecution;

    fn dependencies(_d: &mut ulo::Dependencies) {}

    async fn construct(_r: &ulo::Resolver<'_>) -> Result<Self, ulo::ConstructError> {
        panic!("the constructor panics")
    }
}

impl ulo::Guard<TP> for PanicsOnBuild {
    async fn can_activate(&self, _cx: &PCx) -> Result<bool, BoxError> {
        Ok(true)
    }
}

typed_controller!(Panicking, Singleton, |m| {
    let mut guard = EnhancerSpec::<TP>::new();
    guard.guard_value(G::panicking("pg")).error_handler_value(E::pass("e"));
    m.handler::<TP, _>(HandlerSpec::new("guard", ()).method(guard));

    let mut interceptor = EnhancerSpec::<TP>::new();
    interceptor.interceptor_value(I::panicking("pi")).error_handler_value(E::pass("e"));
    m.handler::<TP, _>(HandlerSpec::new("interceptor", ()).method(interceptor));

    let mut handler = EnhancerSpec::<TP>::new();
    handler.interceptor_value(I::new("oi")).error_handler_value(E::pass("e"));
    m.handler::<TP, _>(HandlerSpec::new("handler", ()).method(handler));

    let mut error_handler = EnhancerSpec::<TP>::new();
    error_handler.error_handler_value(E::pass("e0")).error_handler_value(E::new("e1", Act::Panic));
    m.handler::<TP, _>(HandlerSpec::new("error_handler", ()).method(error_handler));

    let mut build = EnhancerSpec::<TP>::new();
    build.guard::<PanicsOnBuild>().error_handler_value(E::pass("e"));
    m.handler::<TP, _>(HandlerSpec::new("build", ()).method(build));
});

#[tokio::test]
async fn a_panic_is_caught_at_its_stage() {
    use ulo::DispatchStage as S;
    let log = Log::new();
    let l = log.clone();
    let app = connected_on_tokio(Mod::<A>::new(move |m| {
        bind_log(m, &l);
        m.provide::<PanicsOnBuild>();
        m.controller::<Panicking>();
    }))
    .await;

    // (handler, body, the log, the stage the error names)
    let cases: [(&str, Body, &[&str], S); 4] = [
        ("guard", Body::Ok, &["guard pg", "handle e [panic:Guard]"], S::Guard),
        ("interceptor", Body::Ok, &["enter pi", "handle e [panic:Interceptor]"], S::Interceptor),
        (
            "handler",
            Body::Panic,
            // The interceptor around the handler receives the panic as its `next.run()` answer.
            &["enter oi", "handler", "exit oi [panic:Handler]", "handle e [panic:Handler]"],
            S::Handler,
        ),
        // The panicking handler is offered the error first (declared last); the one after it is
        // offered the panic that replaced it.
        ("error_handler", Body::Fail, &["handler", "handle e1 [boom]", "handle e0 [panic:ErrorHandler]"], S::ErrorHandler),
    ];
    for (name, body, expected, stage) in cases {
        let log = Log::new();
        let handler = named::<TP>(&app, name);
        let err = within(name, call(&handler, &log, body)).await.expect_err("a panic answers an error");
        assert_eq!(log.events(), strs(expected), "{name}");
        assert_eq!(stage_of(&err), Some(stage), "{name}");
        assert!(is_panic(&err), "{name}: is_panic reads PanicRecovered");
    }

    // A panic inside a build the container ran is no stage's: it arrives as a lookup error with
    // `Panicked`, and `is_panic` reads that shape too.
    let log = Log::new();
    let handler = named::<TP>(&app, "build");
    let err = within("build", call(&handler, &log, Body::Ok)).await.expect_err("the build panics");
    assert_eq!(log.events(), strs(&["handle e [lookup]"]));
    assert!(err.downcast_ref::<PanicRecovered>().is_none());
    assert!(
        matches!(
            err.downcast_ref::<LookupError>(),
            Some(LookupError::Construct { key, reason: FailureReason::Panicked(_) }) if key.key() == Key::of::<PanicsOnBuild, ()>()
        ),
        "{err}"
    );
    assert!(is_panic(&err));
    drop(handler);
    close(app).await;
}

typed_controller!(Bare, Singleton, |m| m.handler::<TP, _>(HandlerSpec::new("h", ())));

#[tokio::test]
async fn a_global_error_handler_s_panic_is_caught_and_offered_to_the_one_contributed_before_it() {
    let log = Log::new();
    let l = log.clone();
    let app = connected_on_tokio(Mod::<A>::new(move |m| {
        bind_log(m, &l);
        m.enhancer::<AnyErrorHandler<TP>>().value(Arc::new(E::pass("eg0")));
        m.enhancer::<AnyErrorHandler<TP>>().value(Arc::new(E::new("eg1", Act::Panic)));
        m.controller::<Bare>();
    }))
    .await;
    let handler = only::<TP>(&app);
    let call_log = Log::new();
    let err = within("the call", call(&handler, &call_log, Body::Fail)).await.expect_err("nothing claims the panic");
    assert_eq!(call_log.events(), strs(&["handler", "handle eg1 [boom]", "handle eg0 [panic:ErrorHandler]"]));
    assert_eq!(stage_of(&err), Some(ulo::DispatchStage::ErrorHandler));
    drop(handler);
    close(app).await;
}

typed_controller!(ClosurePanics, Singleton, |m| {
    let mut guard = EnhancerSpec::<TP>::new();
    guard
        .guard_with(|| -> std::future::Ready<G> { panic!("the guard closure panics") })
        .error_handler_value(E::pass("e"));
    m.handler::<TP, _>(HandlerSpec::new("guard", ()).method(guard));

    let mut interceptor = EnhancerSpec::<TP>::new();
    interceptor
        .interceptor_with(|| -> std::future::Ready<I> { panic!("the interceptor closure panics") })
        .error_handler_value(E::pass("e"));
    m.handler::<TP, _>(HandlerSpec::new("interceptor", ()).method(interceptor));

    let mut error_handler = EnhancerSpec::<TP>::new();
    error_handler
        .error_handler_value(E::pass("e0"))
        .error_handler_with(|| -> std::future::Ready<E> { panic!("the error handler closure panics") });
    m.handler::<TP, _>(HandlerSpec::new("error_handler", ()).method(error_handler));
});

#[tokio::test]
async fn a_panicking_closure_is_the_stage_of_the_enhancer_it_builds() {
    use ulo::DispatchStage as S;
    let l = Log::new();
    let app = connected_on_tokio(Mod::<A>::new({
        let l = l.clone();
        move |m| {
            bind_log(m, &l);
            m.controller::<ClosurePanics>();
        }
    }))
    .await;
    // (handler, body, the log, the stage the error names)
    let cases: [(&str, Body, &[&str], S); 3] = [
        ("guard", Body::Ok, &["handle e [panic:Guard]"], S::Guard),
        ("interceptor", Body::Ok, &["handle e [panic:Interceptor]"], S::Interceptor),
        // The closure panics when the pipeline builds it, so the handler is offered the panic
        // that replaced the handler's own error.
        ("error_handler", Body::Fail, &["handler", "handle e0 [panic:ErrorHandler]"], S::ErrorHandler),
    ];
    for (name, body, expected, stage) in cases {
        let log = Log::new();
        let handler = named::<TP>(&app, name);
        let err = within(name, call(&handler, &log, body)).await.expect_err("a panic answers an error");
        assert_eq!(log.events(), strs(expected), "{name}");
        assert_eq!(stage_of(&err), Some(stage), "{name}");
        assert!(is_panic(&err), "{name}");
    }
    close(app).await;
}

typed_controller!(Scoped, Singleton, |m| {
    let mut auto_nothing = EnhancerSpec::<TP>::new();
    auto_nothing.guard_with(|log: Dep<Log>| built_guard(&log, "an"));
    m.handler::<TP, _>(HandlerSpec::new("auto_nothing", ()).method(auto_nothing));

    let mut auto_ext = EnhancerSpec::<TP>::new();
    auto_ext.guard_with(|log: Dep<Log>, _u: Ext<User>| built_guard(&log, "ae"));
    m.handler::<TP, _>(HandlerSpec::new("auto_ext", ()).method(auto_ext));

    let mut per_execution = EnhancerSpec::<TP>::new();
    per_execution.guard_with_in::<PerExecution, _, _>(|log: Dep<Log>| built_guard(&log, "pe"));
    m.handler::<TP, _>(HandlerSpec::new("per_execution", ()).method(per_execution));

    let mut transient = EnhancerSpec::<TP>::new();
    transient.guard_with_in::<Transient, _, _>(|log: Dep<Log>| built_guard(&log, "tr"));
    m.handler::<TP, _>(HandlerSpec::new("transient", ()).method(transient));

    let mut singleton = EnhancerSpec::<TP>::new();
    singleton.guard_with_in::<Singleton, _, _>(|log: Dep<Log>| built_guard(&log, "sg"));
    m.handler::<TP, _>(HandlerSpec::new("singleton", ()).method(singleton));
});

async fn scoped(log: &Log) -> ulo::App<ulo::Connected> {
    let l = log.clone();
    connected_on_tokio(Mod::<A>::new(move |m| {
        bind_log(m, &l);
        m.controller::<Scoped>();
    }))
    .await
}

#[tokio::test]
async fn a_closure_is_built_once_unless_it_reads_an_execution() {
    let log = Log::new();
    let app = scoped(&log).await;
    assert_eq!(log.events(), strs(&["build Scoped"]), "a closure is built on first use, not at connect");
    let nothing = named::<TP>(&app, "auto_nothing");
    for _ in 0..3 {
        within("call", call(&nothing, &log, Body::Ok)).await.expect("admits");
    }
    assert_eq!(log.count("build an"), 1, "an Auto closure reading nothing is built once and shared");
    assert_eq!(log.count("guard an"), 3);

    let ext = named::<TP>(&app, "auto_ext");
    for _ in 0..3 {
        within("call", call(&ext, &log, Body::Ok)).await.expect("admits");
    }
    assert_eq!(log.count("build ae"), 3, "an Auto closure reading Ext is built per execution");
    drop((nothing, ext));
    close(app).await;
}

#[tokio::test]
async fn an_explicit_scope_overrides_the_closure_inference() {
    let log = Log::new();
    let app = scoped(&log).await;

    // Reading nothing, a `PerExecution` closure is still built per execution, and once inside one.
    let per_execution = named::<TP>(&app, "per_execution");
    for _ in 0..3 {
        within("call", call(&per_execution, &log, Body::Ok)).await.expect("admits");
    }
    assert_eq!(log.count("build pe"), 3);
    {
        let exec = open(&per_execution);
        for _ in 0..2 {
            within("call", call_in(&per_execution, &exec, &log, Body::Ok)).await.expect("admits");
        }
    }
    assert_eq!(log.count("build pe"), 4, "two dispatches in one execution share the build");

    // A `Transient` closure is built each time the pipeline obtains it, so twice in one execution.
    let transient = named::<TP>(&app, "transient");
    {
        let exec = open(&transient);
        for _ in 0..2 {
            within("call", call_in(&transient, &exec, &log, Body::Ok)).await.expect("admits");
        }
    }
    assert_eq!(log.count("build tr"), 2);

    // An explicit `Singleton` closure is built once.
    let singleton = named::<TP>(&app, "singleton");
    for _ in 0..3 {
        within("call", call(&singleton, &log, Body::Ok)).await.expect("admits");
    }
    assert_eq!(log.count("build sg"), 1);
    drop((per_execution, transient, singleton));
    close(app).await;
}

typed_controller!(Valued, Singleton, |m| {
    // One value built here, handed to both handlers' controller tier, as `#[routes]` does for an
    // impl-level `value = expr`; each handler's own method-tier value is a value of its own.
    let shared = Arc::new(Counting::new("limit"));
    for (name, own) in [("first", "own1"), ("second", "own2")] {
        let mut controller = EnhancerSpec::<TP>::new();
        controller.guard_arc(Arc::clone(&shared));
        let mut method = EnhancerSpec::<TP>::new();
        method.guard_value(Counting::new(own));
        m.handler::<TP, _>(HandlerSpec::new(name, ()).controller(controller).method(method));
    }
});

#[tokio::test]
async fn a_value_is_built_once_and_a_controller_level_value_is_shared_by_every_handler() {
    let log = Log::new();
    let l = log.clone();
    let app = connected_on_tokio(Mod::<A>::new(move |m| {
        bind_log(m, &l);
        m.controller::<Valued>();
    }))
    .await;
    let first = named::<TP>(&app, "first");
    let second = named::<TP>(&app, "second");
    for handler in [&first, &first, &second] {
        within("call", call(handler, &log, Body::Ok)).await.expect("admits");
    }
    // The controller-level value counts all three calls; each method-level value counts its own
    // handler's calls, across executions.
    assert_eq!(
        after(&log, "serve"),
        strs(&["limit 1", "own1 1", "limit 2", "own1 2", "limit 3", "own2 1"]),
        "{:?}",
        log.events()
    );
    drop((first, second));
    close(app).await;
}

typed_guard!(SingletonG, Singleton);
typed_guard!(PlainAuto, Auto);
typed_guard!(ExtG, Auto, reads Ext<User>);
typed_guard!(ExecG, PerExecution);
typed_guard!(TransientG, Transient);

typed_controller!(Typed, Singleton, |m| {
    let mut method = EnhancerSpec::<TP>::new();
    method.guard::<SingletonG>().guard::<PlainAuto>().guard::<ExtG>().guard::<ExecG>().guard::<TransientG>();
    m.handler::<TP, _>(HandlerSpec::new("h", ()).method(method));
});

#[tokio::test]
async fn a_type_token_is_resolved_from_the_container_in_the_scope_its_type_declares() {
    let log = Log::new();
    let l = log.clone();
    let app = connected_on_tokio(Mod::<A>::new(move |m| {
        bind_log(m, &l);
        m.provide::<SingletonG>();
        m.provide::<PlainAuto>();
        m.provide::<ExtG>();
        m.provide::<ExecG>();
        m.provide::<TransientG>();
        m.controller::<Typed>();
    }))
    .await;
    // An enhancer that reads no execution data is a singleton, `Auto` or declared, and is built
    // with the graph; the others wait for a call.
    assert_eq!(log.events(), strs(&["build SingletonG", "build PlainAuto", "build Typed"]));

    let handler = only::<TP>(&app);
    within("call", call(&handler, &log, Body::Ok)).await.expect("admits");
    within("call", call(&handler, &log, Body::Ok)).await.expect("admits");
    assert_eq!(log.count("build SingletonG"), 1);
    assert_eq!(log.count("build PlainAuto"), 1);
    assert_eq!(log.count("build ExtG"), 2, "Auto over an Ext read is built per execution");
    assert_eq!(log.count("build ExecG"), 2);
    assert_eq!(log.count("build TransientG"), 2);
    assert_eq!(log.count("guard SingletonG"), 2, "the one instance runs on every call");

    // Two dispatches in one execution: the per-execution instances are shared, a transient is not.
    let exec = open(&handler);
    for _ in 0..2 {
        within("call", call_in(&handler, &exec, &log, Body::Ok)).await.expect("admits");
    }
    assert_eq!(log.count("build ExtG"), 3);
    assert_eq!(log.count("build ExecG"), 3);
    assert_eq!(log.count("build TransientG"), 4);
    drop((handler, exec));
    close(app).await;
}
