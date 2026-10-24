//! Runtime lookups (section 10.2): each `LookupError` variant a caller can branch on, answered from
//! the app, from a module handle and from inside an execution.
//!
//! Capabilities pinned: 47 (lookups work from the application, a standalone context or a module
//! handle, with or without an execution, and answer a typed error: not found, wrong type,
//! execution required, ambiguous module), 37 (the handle's visibility and `with_execution`, the
//! lookup half), 10 (a single read as a collection and the reverse). The runtime rows of
//! section 3: `NotReady` from a hook-time lookup, `Closed` from Destroying on, `is_panic` through
//! `LookupError::Construct`.
//!
//! Assertions are on variants and fields; `NotFound` for an extension or an input is in
//! `executions.rs`.

#[macro_use]
mod common;

use std::sync::{Arc, Mutex};

use common::*;
use ulo::{ExecOptions, FailureReason, Key, LookupError, LookupKind, Module, ModuleDef, ModuleIdentity, ModuleRef};

struct A;
struct P;
struct Q;
struct Svc;
struct Other;

trait Plugin: Send + Sync {}
struct P1;
impl Plugin for P1 {}

#[tokio::test]
async fn an_unbound_key_and_an_unregistered_module_are_not_found() {
    let app = connected(Mod::<A>::bare()).await;
    match app.get::<Svc>().await {
        Err(LookupError::NotFound { key, kind: LookupKind::Binding }) => assert_eq!(key.key(), Key::of::<Svc, ()>()),
        other => panic!("expected NotFound(Binding), found {:?}", other.map(|_| ())),
    }
    match app.module::<Svc>() {
        Err(LookupError::NotFound { kind: LookupKind::Module, .. }) => {}
        other => panic!("expected NotFound(Module), found {:?}", other.map(|_| ())),
    }
    close(app).await;
}

#[tokio::test]
async fn a_singleton_read_before_the_walk_built_it_is_not_ready() {
    struct Early(bool);
    struct Late;
    let app = connected(Mod::<A>::new(|m| {
        m.singleton(|module: ModuleRef| async move {
            let read = module.get::<Late>().await;
            Early(matches!(read, Err(LookupError::NotReady { .. })))
        });
        m.singleton(|| async { Late });
    }))
    .await;
    assert!(app.get::<Early>().await.unwrap().0, "a constructor asking for a singleton the walk has not reached");
    assert!(app.get::<Late>().await.is_ok(), "and once connect returns it cannot occur");
    close(app).await;
}

#[tokio::test]
async fn a_key_asked_for_as_another_type_is_wrong_type_and_builds_nothing() {
    let built = Count::new();
    let c = built.clone();
    let app = connected(Mod::<A>::new(move |m| {
        let c = c.clone();
        m.execution(move || {
            c.bump();
            async { Svc }
        });
    }))
    .await;
    let outcome = app
        .execute(ExecOptions::new(), async |exec| exec.resolver().by_key::<u8>(Key::of::<Svc, ()>()).await.map(|_| ()))
        .await
        .unwrap();
    match outcome {
        Err(LookupError::WrongType { key, requested }) => {
            assert_eq!(key.key(), Key::of::<Svc, ()>());
            assert!(requested.contains("u8"), "{requested}");
        }
        other => panic!("expected WrongType, found {other:?}"),
    }
    assert_eq!(built.get(), 0, "the type is checked before anything is built");
    close(app).await;
}

#[tokio::test]
async fn a_single_read_as_a_collection_and_a_collection_read_as_a_single_are_wrong_kind() {
    use ulo::BindingKind::{Collection, Single};
    let app = connected(Mod::<A>::new(|m| {
        m.value(P1).also_as::<dyn Plugin>(|a| a);
        m.contribute::<dyn std::any::Any + Send + Sync>().value(Arc::new(1_u8));
    }))
    .await;
    let outcome = app.execute(ExecOptions::new(), async |exec| exec.resolver().many::<dyn Plugin>().await.map(|_| ())).await.unwrap();
    match outcome {
        Err(LookupError::WrongKind { expected, found, .. }) => assert_eq!((expected, found), (Collection, Single)),
        other => panic!("expected WrongKind, found {other:?}"),
    }
    match app.get::<dyn std::any::Any + Send + Sync>().await {
        Err(LookupError::WrongKind { expected, found, .. }) => assert_eq!((expected, found), (Single, Collection)),
        other => panic!("expected WrongKind, found {:?}", other.map(|_| ())),
    }
    close(app).await;
}

#[tokio::test]
async fn an_execution_scoped_binding_needs_an_execution_from_the_app_and_a_module_handle() {
    let app = connected(Mod::<A>::new(|m| {
        m.execution(|| async { Svc });
    }))
    .await;
    let module = app.module::<Mod<A>>().unwrap();
    assert!(matches!(module.get::<Svc>().await, Err(LookupError::ExecutionRequired { .. })));
    assert!(matches!(app.get::<Svc>().await, Err(LookupError::ExecutionRequired { .. })));
    app.execute(ExecOptions::new(), async |exec| {
        let inside = module.with_execution(&exec.handle());
        assert!(inside.get::<Svc>().await.is_ok(), "the handle carrying an execution reaches it");
    })
    .await
    .unwrap();
    close(app).await;
}

#[tokio::test]
async fn a_module_handle_sees_only_what_its_module_sees() {
    let app = connected(Mod::<A>::new(|m| {
        m.import(Mod::<P>::new(|m| {
            m.value(Svc);
        }));
    }))
    .await;
    let inner = app.module::<Mod<P>>().unwrap();
    assert!(inner.get::<Svc>().await.is_ok(), "the module's own binding");
    match app.module::<Mod<A>>().unwrap().get::<Svc>().await {
        Err(LookupError::NotFound { kind: LookupKind::Binding, .. }) => {}
        other => panic!("an unexported binding is invisible to the importer: {:?}", other.map(|_| ())),
    }
    close(app).await;
}

#[tokio::test]
async fn a_key_two_exports_answer_is_ambiguous_from_a_module_that_sees_both() {
    let app = connected(Mod::<A>::new(|m| {
        m.import(Mod::<Q>::new(|m| {
            m.import(Mod::<P>::new(|m| {
                m.value(Svc);
                m.export::<Svc>();
            }));
            m.import(Mod::<Other>::new(|m| {
                m.value(Svc);
                m.export::<Svc>();
            }));
        }));
    }))
    .await;
    match app.module::<Mod<Q>>().unwrap().get::<Svc>().await {
        Err(LookupError::Ambiguous { key, sources }) => {
            assert_eq!(key.key(), Key::of::<Svc, ()>());
            assert_eq!(sources.iter().map(ToString::to_string).collect::<Vec<_>>(), [mod_name::<P>(), mod_name::<Other>()]);
        }
        other => panic!("expected Ambiguous, found {:?}", other.map(|_| ())),
    }
    close(app).await;
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct Conf(u8);

impl Module for Conf {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_value(self)
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.value(Svc);
    }
}

#[tokio::test]
async fn a_module_type_with_two_configurations_is_an_ambiguous_module() {
    let app = connected(Mod::<A>::new(|m| {
        m.import(Conf(1));
        m.import(Conf(2));
        m.import(Conf(1).keyed::<P>());
        m.import(Conf(1).keyed::<Q>());
    }))
    .await;
    match app.module::<Conf>() {
        Err(LookupError::AmbiguousModule { candidates, .. }) => assert_eq!(candidates.len(), 4),
        other => panic!("expected AmbiguousModule, found {:?}", other.map(|_| ())),
    }
    assert!(app.module_keyed::<Conf, P>().is_ok() && app.module_keyed::<Conf, Q>().is_ok(), "a key names one");
    close(app).await;
}

#[tokio::test]
async fn a_failing_or_panicking_build_inside_a_call_is_a_construct_error() {
    struct Fails;
    struct Panics;
    let app = connected(Mod::<A>::new(|m| {
        m.try_execution(|| async { Err::<Fails, _>(Boom::new("no")) });
        m.execution(|| async {
            if true {
                panic!("boom");
            }
            Panics
        });
    }))
    .await;
    app.execute(ExecOptions::new(), async |exec| {
        match exec.get::<Fails>().await {
            Err(LookupError::Construct { key, reason: FailureReason::Errored(_) }) => assert_eq!(key.key(), Key::of::<Fails, ()>()),
            other => panic!("expected Construct(Errored), found {:?}", other.map(|_| ())),
        }
        let panicked = exec.get::<Panics>().await.map(|_| ()).unwrap_err();
        assert!(matches!(&panicked, LookupError::Construct { reason: FailureReason::Panicked(_), .. }), "{panicked:?}");
        assert!(ulo::is_panic(&(Box::new(panicked) as ulo::BoxError)), "is_panic finds it through the lookup error");
        let errored = exec.get::<Fails>().await.map(|_| ()).unwrap_err();
        assert!(!ulo::is_panic(&(Box::new(errored) as ulo::BoxError)));
    })
    .await
    .unwrap();
    close(app).await;
}

#[tokio::test]
async fn a_singleton_lookup_from_destroying_on_is_closed() {
    let seen: Arc<Mutex<Option<bool>>> = Arc::default();
    let s = Arc::clone(&seen);
    let app = connected(Mod::<A>::new(move |m| {
        let s = Arc::clone(&s);
        m.value(Svc);
        m.on_destroy(move |module: ModuleRef| {
            let s = Arc::clone(&s);
            async move {
                let read = module.get::<Svc>().await;
                *s.lock().unwrap() = Some(matches!(read, Err(LookupError::Closed { .. })));
            }
        });
    }))
    .await;
    let handle = app.handle();
    assert!(handle.get::<Svc>().await.is_ok());
    close(app).await;
    assert_eq!(*seen.lock().unwrap(), Some(true), "inside a destroy hook");
    assert!(matches!(handle.get::<Svc>().await, Err(LookupError::Closed { .. })), "and after the close");
}
