//! Roles and where controllers live: a role comes from the trait an enhancer implements and the
//! transport it names; a binding under a role key registers that role; a controller is a module's
//! binding whose handlers see what the module sees; the three role strings of
//! `ClosureScopeViolation`.
//!
//! Capabilities pinned: 25 (an enhancer for one transport is not one for another: the role keys
//! `AnyGuard`, `AnyInterceptor` and `AnyErrorHandler` are per transport), 28 (a binding under a
//! role key registers the role, a key no handler reads and no `enhancer` marks does not), 29 (the
//! controller half: a module declares controllers, their handlers resolve with that module's
//! visibility, and an export is the only way a controller binding leaves), 26 (the closure scope
//! refusal, `interceptor` and `error handler` roles), 44.
//!
//! The pipeline's order, laziness and error rows are in `enhancers.rs`.

#[macro_use]
mod common;

use std::future::ready;
use std::sync::Arc;

use common::pipeline::*;
use common::*;
use ulo::scope::{Auto, Singleton};
use ulo::{
    AnyErrorHandler, AnyGuard, AnyInterceptor, Dep, EnhancerSpec, Execution, ExecOptions, Ext, HandlerSpec, Key, LookupError,
    LookupKind, TypeName, WiringError,
};

struct A;
struct X;
struct Y;

struct L;
struct Mark;
struct Use;

fn after(log: &Log, verb: &str) -> Vec<String> {
    let prefix = format!("{verb} ");
    log.events().into_iter().filter_map(|line| line.strip_prefix(&prefix).map(str::to_owned)).collect()
}

fn strs(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_owned()).collect()
}

typed_guard!(OnlyP, Singleton);

typed_controller!(Both, Singleton, |m| {
    m.handler::<TP, _>(HandlerSpec::new("p", ()).route("p"));
    m.handler::<TQ, _>(HandlerSpec::new("q", ()).route("q"));
});

#[tokio::test]
async fn an_enhancer_for_one_transport_is_not_one_for_another() {
    let log = Log::new();
    let l = log.clone();
    let app = connected_on_tokio(Mod::<A>::new(move |m| {
        bind_log(m, &l);
        // `OnlyP` implements `Guard<TP>` and nothing else: it can be contributed under
        // `AnyGuard<TP>`, and the key `AnyGuard<TQ>` is a different role.
        m.enhancer::<AnyGuard<TP>>().provide::<OnlyP>(|a| a);
        m.enhancer::<AnyGuard<TP>>().value(Arc::new(G::new("gp")));
        m.enhancer::<AnyGuard<TQ>>().value(Arc::new(G::new("gq")));
        m.enhancer::<AnyInterceptor<TP>>().value(Arc::new(I::new("ip")));
        m.enhancer::<AnyErrorHandler<TP>>().value(Arc::new(E::pass("ep")));
        m.enhancer::<AnyErrorHandler<TQ>>().value(Arc::new(E::pass("eq")));
        m.controller::<Both>();
    }))
    .await;
    let p = named::<TP>(&app, "p");
    let q = named::<TQ>(&app, "q");
    assert_eq!(p.info().transport(), "p");
    assert_eq!(q.info().transport(), "q");

    within("p", call(&p, &log, Body::Fail)).await.expect_err("nothing claims the error");
    assert_eq!(after(&log, "guard"), strs(&["OnlyP", "gp"]));
    assert_eq!(after(&log, "enter"), strs(&["ip"]));
    assert_eq!(after(&log, "handle"), strs(&["ep [boom]"]));

    let log_q = Log::new();
    within("q", call(&q, &log_q, Body::Fail)).await.expect_err("nothing claims the error");
    // Nothing the TP side contributed ran on the TQ handler, and the reverse.
    assert_eq!(after(&log_q, "guard"), strs(&["gq"]));
    assert!(after(&log_q, "enter").is_empty(), "no interceptor is contributed for TQ");
    assert_eq!(after(&log_q, "handle"), strs(&["eq [boom]"]));
    drop((p, q));
    close(app).await;
}

typed_controller!(One, Singleton, |m| m.handler::<TP, _>(HandlerSpec::new("h", ())));

trait Plugin: Send + Sync {}

struct PlugA;

impl Plugin for PlugA {}

#[tokio::test]
async fn a_contribution_under_a_role_key_a_handler_reads_is_an_enhancer() {
    let log = Log::new();
    let l = log.clone();
    // Contributed with the plain `contribute`, under the role key `AnyGuard<TP>`, which the
    // mounted handler's transport reads: the closure reads per-execution data, which only an
    // enhancer may, and is built per execution.
    let app = connected_on_tokio(Mod::<A>::new(move |m| {
        bind_log(m, &l);
        m.contribute::<AnyGuard<TP>>()
            .with(|log: Dep<Log>, _u: Ext<User>| built_guard(&log, "cg"), |a| a);
        m.controller::<One>();
    }))
    .await;
    let handler = only::<TP>(&app);
    for _ in 0..2 {
        within("call", call(&handler, &log, Body::Ok)).await.expect("admits");
    }
    assert_eq!(log.count("build cg"), 2, "built per execution");
    assert_eq!(log.count("guard cg"), 2);
    drop(handler);
    close(app).await;
}

#[test]
fn the_same_contribution_under_a_key_that_is_not_a_role_is_a_provider() {
    let errors = wire_err(Mod::<A>::new(|m| {
        m.contribute::<dyn Plugin>().with(|_u: Ext<User>| async { PlugA }, |a| a);
    }));
    let (binding, by_closure) = entry!(errors, WiringError::ScopeViolation { binding, by_closure, .. } => (binding.key(), *by_closure));
    assert_eq!(binding, Key::of::<dyn Plugin, ()>(), "the collection the contribution joins");
    assert!(by_closure);
    assert_eq!(errors.len(), 1, "{errors}");
}

#[test]
fn a_role_key_no_handler_reads_is_a_role_only_when_something_marks_it() {
    // No handler of `TQ` is mounted, so `AnyGuard<TQ>` is outside the set of role keys: a plain
    // contribution reading `Ext` is a provider contribution, refused as a singleton would be.
    let errors = wire_err(Mod::<A>::new(|m| {
        m.contribute::<AnyGuard<TQ>>().with(|_u: Ext<User>| ready(G::new("q")), |a| a);
    }));
    entry!(errors, WiringError::ScopeViolation { .. } => ());
    assert_eq!(errors.len(), 1, "{errors}");

    // `enhancer` marks the key, so the same closure is an enhancer built per execution.
    wires(Mod::<A>::new(|m| {
        m.contribute::<AnyGuard<TQ>>().with(|_u: Ext<User>| ready(G::new("q")), |a| a);
        m.enhancer::<AnyGuard<TQ>>().value(Arc::new(G::new("marked")));
    }));

    // One marked contribution marks the whole key, whichever module wrote it.
    wires(Mod::<A>::new(|m| {
        m.import(Mod::<Mark>::new(|m| {
            m.enhancer::<AnyGuard<TQ>>().value(Arc::new(G::new("marked")));
        }));
        m.import(Mod::<Use>::new(|m| {
            m.contribute::<AnyGuard<TQ>>().with(|_u: Ext<User>| ready(G::new("q")), |a| a);
        }));
    }));
}

struct PrivX;

typed_controller!(CtlX, Singleton, |m| {
    m.handler::<TP, _>(HandlerSpec::new("x", ()).route("/x"));
});

typed_controller!(CtlY, Singleton, |m| {
    m.handler::<TP, _>(HandlerSpec::new("y1", ()).route("/y1"));
    m.handler::<TQ, _>(HandlerSpec::new("y2", ()).route("y2"));
});

fn logs(log: &Log) -> Mod<L> {
    let log = log.clone();
    Mod::<L>::new(move |m| {
        m.global();
        m.value(log.clone());
        m.export::<Log>();
    })
}

#[tokio::test]
async fn a_module_s_controllers_mount_and_their_handlers_see_what_that_module_sees() {
    let log = Log::new();
    let l = log.clone();
    let app = connected_on_tokio(Mod::<A>::new(move |m| {
        m.import(logs(&l));
        m.import(Mod::<X>::new(|m| {
            m.execution(|| ready(PrivX));
            m.controller::<CtlX>();
        }));
        m.import(Mod::<Y>::new(|m| {
            m.controller::<CtlY>();
        }));
    }))
    .await;

    let mut infos: Vec<_> = app
        .handle()
        .handlers()
        .iter()
        .map(|info| (info.name(), info.transport(), info.route().map(str::to_owned), info.controller().key()))
        .collect();
    infos.sort_by_key(|info| info.0);
    assert_eq!(
        infos,
        vec![
            ("x", "p", Some("/x".to_owned()), Key::of::<CtlX, ()>()),
            ("y1", "p", Some("/y1".to_owned()), Key::of::<CtlY, ()>()),
            ("y2", "q", Some("y2".to_owned()), Key::of::<CtlY, ()>()),
        ]
    );

    let x = named::<TP>(&app, "x");
    let y1 = named::<TP>(&app, "y1");
    let y2 = named::<TQ>(&app, "y2");
    let module_x = app.handle().module::<Mod<X>>().expect("one X").name();
    let module_y = app.handle().module::<Mod<Y>>().expect("one Y").name();
    assert_eq!(x.module().name(), module_x);
    assert_eq!(y1.module().name(), module_y);
    assert_eq!(y2.module().name(), module_y, "both transports' handlers of one controller share its module");

    // The module a handler is mounted in is the visibility its calls resolve with: `PrivX` is X's
    // alone.
    let in_x = Execution::open(x.module(), ExecOptions::new()).expect("running");
    assert!(in_x.get::<PrivX>().await.is_ok());
    let in_y = Execution::open(y1.module(), ExecOptions::new()).expect("running");
    assert!(matches!(
        in_y.get::<PrivX>().await,
        Err(LookupError::NotFound { kind: LookupKind::Binding, .. })
    ));
    drop((in_x, in_y, x, y1, y2));
    close(app).await;
}

#[test]
fn a_controller_leaves_its_module_only_when_exported() {
    let consumer = |m: &mut ulo::ModuleDef<'_>| {
        m.singleton(|_c: Dep<CtlX>| async { Hidden });
    };
    let errors = wire_err(Mod::<A>::new(move |m| {
        m.import(logs(&Log::new()));
        m.import(Mod::<X>::new(|m| {
            m.controller::<CtlX>();
        }));
        consumer(m);
    }));
    let key = entry!(errors, WiringError::Missing { key, .. } => key.key());
    assert_eq!(key, Key::of::<CtlX, ()>());
    assert_eq!(errors.len(), 1, "{errors}");

    wires(Mod::<A>::new(move |m| {
        m.import(logs(&Log::new()));
        m.import(Mod::<X>::new(|m| {
            m.controller::<CtlX>();
            m.export::<CtlX>();
        }));
        consumer(m);
    }));
}

struct Hidden;

macro_rules! closure_enhancer_controller {
    ($name:ident, |$c:ident, $m:ident| $body:expr) => {
        struct $name;

        impl ulo::Construct for $name {
            type Scope = Auto;

            fn dependencies(_d: &mut ulo::Dependencies) {}

            async fn construct(_r: &ulo::Resolver<'_>) -> Result<Self, ulo::ConstructError> {
                Ok($name)
            }
        }

        impl ulo::Controller for $name {
            fn mount(m: &mut ulo::Mount<'_>) {
                let mut $c = EnhancerSpec::<TP>::new();
                let mut $m = EnhancerSpec::<TP>::new();
                $body;
                m.handler::<TP, _>(HandlerSpec::new("h", ()).controller($c).method($m));
            }
        }
    };
}

closure_enhancer_controller!(SingletonInterceptors, |c, m| {
    c.interceptor_with_in::<Singleton, _, _>(|_u: Ext<User>| ready(I::new("c")));
    // Every form counts toward the position: the value is the method tier's first interceptor, the
    // refused closure its second.
    m.interceptor_value(I::new("v"));
    m.interceptor_with_in::<Singleton, _, _>(|_u: Ext<User>| ready(I::new("m")));
});

closure_enhancer_controller!(SingletonErrorHandlers, |c, m| {
    c.error_handler_value(E::pass("v"));
    c.error_handler_with_in::<Singleton, _, _>(|_u: Ext<User>| ready(E::pass("c")));
    m.error_handler_with_in::<Singleton, _, _>(|_u: Ext<User>| ready(E::pass("m")));
});

#[test]
fn an_explicit_singleton_closure_is_refused_in_the_interceptor_and_error_handler_roles_too() {
    let errors = wire_err(Mod::<A>::new(|m| {
        m.controller::<SingletonInterceptors>();
        m.controller::<SingletonErrorHandlers>();
    }));
    let found = entries!(errors, WiringError::ClosureScopeViolation { closure, transport, role, path, at }
        => (closure.clone(), *transport, *role, path.clone(), *at));
    assert_eq!(found.len(), 4, "{errors}");
    assert_eq!(errors.len(), 4, "{errors}");
    for (_, transport, _, path, at) in &found {
        assert_eq!(*transport, TypeName::of::<TP>());
        assert!(path.iter().any(|step| step.contains("Ext<")), "{path:?}");
        assert_eq!(at.file(), file!(), "`at` points at the declaration");
    }
    let mut lines: Vec<u32> = found.iter().map(|f| f.4.line()).collect();
    lines.sort_unstable();
    lines.dedup();
    assert_eq!(lines.len(), 4, "each entry points at its own declaration: {found:?}");
    let by_role = |role: &str| -> Vec<&String> { found.iter().filter(|f| f.2 == role).map(|f| &f.0).collect() };
    let interceptors = by_role("interceptor");
    let handlers = by_role("error handler");
    assert_eq!(interceptors.len(), 2, "{found:?}");
    assert_eq!(handlers.len(), 2, "{found:?}");
    assert!(by_role("guard").is_empty());
    // The closure label carries the tier and the position among the tier's declarations of the
    // role: the method tier's refused interceptor and the controller tier's refused error handler
    // each follow a value, so no two labels of one role share a position.
    assert!(interceptors.iter().any(|c| c.starts_with("controller-level interceptor #1")), "{interceptors:?}");
    assert!(interceptors.iter().any(|c| c.starts_with("method-level interceptor #2")), "{interceptors:?}");
    assert!(handlers.iter().any(|c| c.starts_with("controller-level error handler #2")), "{handlers:?}");
    assert!(handlers.iter().any(|c| c.starts_with("method-level error handler #1")), "{handlers:?}");
}
