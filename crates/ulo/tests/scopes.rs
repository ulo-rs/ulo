//! Scopes at run time (section 6): what is built when, and who shares it.
//!
//! Capabilities pinned: 17 (every holder of a singleton holds the same non-`Clone` object, a
//! mutation and an init hook's effect visible to all), 18 (a singleton is built once at connect),
//! 19 (an execution-scoped binding is built once per execution and shared inside it; released when
//! the execution ends), 20 (a transient is built at every site), 22 (a controller reading
//! execution data, directly or through a dependency, is built per call; one that reads none is
//! built once), and the `Auto` rule for a provider and a controller.
//!
//! A construction counter is a [`Count`] a factory bumps, or a static a hand-written `Construct`
//! bumps (the trait function has no `self` to carry one). Controllers are called through
//! `dispatch` over the handlers `AppHandle::mounted` hands out, which needs a runtime.

#[macro_use]
mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use common::*;
use ulo::scope::{Auto, PerExecution};
use ulo::{App, Connected, Dep, ExecOptions, Execution, Ext, HandlerSpec, LookupError, MountedHandler};

struct A;
struct User;
struct Svc;

struct Shared {
    n: Mutex<u32>,
}

struct H1(Dep<Shared>);
struct H2(Dep<Shared>);

#[tokio::test]
async fn two_holders_share_one_non_clone_singleton() {
    let app = connected(Mod::<A>::new(|m| {
        m.singleton(|| async { Shared { n: Mutex::new(1) } })
            .on_init(|shared: Dep<Shared>| async move {
                *shared.n.lock().unwrap() += 10;
                Ok::<(), Boom>(())
            });
        m.singleton(|shared: Dep<Shared>| async move { H1(shared) });
        m.singleton(|shared: Dep<Shared>| async move { H2(shared) });
    }))
    .await;
    let (h1, h2, direct) = (app.get::<H1>().await.unwrap(), app.get::<H2>().await.unwrap(), app.get::<Shared>().await.unwrap());
    assert!(Arc::ptr_eq(h1.0.arc(), h2.0.arc()), "two holders, one object");
    assert!(Arc::ptr_eq(h1.0.arc(), direct.arc()), "and it is the object a lookup returns");
    assert_eq!(*h1.0.n.lock().unwrap(), 11, "the init hook ran once, on the shared object");
    *h1.0.n.lock().unwrap() += 1;
    assert_eq!(*h2.0.n.lock().unwrap(), 12, "one holder's mutation is the other's");
    close(app).await;
}

#[tokio::test]
async fn a_singleton_is_built_once_at_connect_and_shared() {
    let built = Count::new();
    let c = built.clone();
    let app = connected(Mod::<A>::new(move |m| {
        let c = c.clone();
        m.singleton(move || {
            c.bump();
            async { Svc }
        });
    }))
    .await;
    assert_eq!(built.get(), 1, "built during connect, before anything asked");

    let first = app.get::<Svc>().await.unwrap();
    for _ in 0..3 {
        assert!(Arc::ptr_eq(first.arc(), app.get::<Svc>().await.unwrap().arc()));
    }
    for _ in 0..3 {
        let same = app.execute(ExecOptions::new(), async |exec| exec.get::<Svc>().await.unwrap().into_arc()).await.unwrap();
        assert!(Arc::ptr_eq(first.arc(), &same), "an execution reads the same singleton");
    }
    assert_eq!(built.get(), 1, "gets and executions build nothing");
    close(app).await;
}

#[tokio::test]
async fn an_auto_provider_that_reads_no_execution_data_is_a_singleton() {
    let built = Count::new();
    let c = built.clone();
    let app = connected(Mod::<A>::new(move |m| {
        let c = c.clone();
        m.with(move || {
            c.bump();
            async { Svc }
        });
    }))
    .await;
    assert_eq!(built.get(), 1, "an `Auto` provider is built at connect");
    let (a, b) = (app.get::<Svc>().await.unwrap(), app.get::<Svc>().await.unwrap());
    assert!(Arc::ptr_eq(a.arc(), b.arc()));
    assert_eq!(built.get(), 1);
    close(app).await;
}

struct Reader1(Dep<Svc>);
struct Reader2(Dep<Svc>);

#[tokio::test]
async fn an_execution_scoped_binding_is_built_once_per_execution() {
    let built = Count::new();
    let c = built.clone();
    let app = connected(Mod::<A>::new(move |m| {
        let c = c.clone();
        m.execution(move || {
            c.bump();
            async { Svc }
        });
        m.execution(|svc: Dep<Svc>| async move { Reader1(svc) });
        m.execution(|svc: Dep<Svc>| async move { Reader2(svc) });
    }))
    .await;
    assert_eq!(built.get(), 0, "nothing is built for an execution before one opens");

    let first = app
        .execute(ExecOptions::new(), async |exec| {
            let (a, b) = (exec.get::<Svc>().await.unwrap(), exec.get::<Svc>().await.unwrap());
            let (r1, r2) = (exec.get::<Reader1>().await.unwrap(), exec.get::<Reader2>().await.unwrap());
            assert!(Arc::ptr_eq(a.arc(), b.arc()), "two reads in one execution");
            assert!(Arc::ptr_eq(a.arc(), r1.0.arc()) && Arc::ptr_eq(a.arc(), r2.0.arc()), "and two holders in it");
            a.into_arc()
        })
        .await
        .unwrap();
    assert_eq!(built.get(), 1);
    let second = app.execute(ExecOptions::new(), async |exec| exec.get::<Svc>().await.unwrap().into_arc()).await.unwrap();
    assert_eq!(built.get(), 2, "the second execution built its own");
    assert!(!Arc::ptr_eq(&first, &second));
    close(app).await;
}

#[tokio::test]
async fn an_execution_scoped_binding_cannot_be_read_without_an_execution() {
    let app = connected(Mod::<A>::new(|m| {
        m.execution(|| async { Svc });
    }))
    .await;
    assert!(matches!(app.get::<Svc>().await, Err(LookupError::ExecutionRequired { .. })));
    close(app).await;
}

struct Tracked(Arc<AtomicUsize>);

impl Drop for Tracked {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn execution_scoped_instances_are_released_when_the_execution_drops() {
    let dropped = Arc::new(AtomicUsize::new(0));
    let d = Arc::clone(&dropped);
    let app = connected(Mod::<A>::new(move |m| {
        let d = Arc::clone(&d);
        m.execution(move || {
            let d = Arc::clone(&d);
            async move { Tracked(d) }
        });
    }))
    .await;

    app.execute(ExecOptions::new(), async |exec| {
        exec.get::<Tracked>().await.unwrap();
    })
    .await
    .unwrap();
    assert_eq!(dropped.load(Ordering::SeqCst), 1, "released when `execute` returns");

    let handle = app
        .execute(ExecOptions::new(), async |exec| {
            exec.get::<Tracked>().await.unwrap();
            exec.handle()
        })
        .await
        .unwrap();
    assert_eq!(dropped.load(Ordering::SeqCst), 2, "released when `execute` returns, a handle kept past it notwithstanding");
    assert!(matches!(handle.get::<Tracked>().await, Err(LookupError::ExecutionEnded { .. })), "the handle builds no second instance");
    drop(handle);
    assert_eq!(dropped.load(Ordering::SeqCst), 2);
    close(app).await;
}

struct Tr;
struct E1(Dep<Tr>);
struct E2(Dep<Tr>);
struct S1(Dep<Tr>);
struct S2(Dep<Tr>);

#[tokio::test]
async fn a_transient_is_built_at_every_site() {
    let built = Count::new();
    let c = built.clone();
    let app = connected(Mod::<A>::new(move |m| {
        let c = c.clone();
        m.transient(move || {
            c.bump();
            async { Tr }
        });
        m.execution(|tr: Dep<Tr>| async move { E1(tr) });
        m.execution(|tr: Dep<Tr>| async move { E2(tr) });
    }))
    .await;
    assert_eq!(built.get(), 0, "nothing reads it at connect");

    app.execute(ExecOptions::new(), async |exec| {
        let (e1, e2) = (exec.get::<E1>().await.unwrap(), exec.get::<E2>().await.unwrap());
        assert!(!Arc::ptr_eq(e1.0.arc(), e2.0.arc()), "two holders in one execution, two instances");
        let (a, b) = (exec.get::<Tr>().await.unwrap(), exec.get::<Tr>().await.unwrap());
        assert!(!Arc::ptr_eq(a.arc(), b.arc()), "and two reads");
    })
    .await
    .unwrap();
    assert_eq!(built.get(), 4);
    let (a, b) = (app.get::<Tr>().await.unwrap(), app.get::<Tr>().await.unwrap());
    assert!(!Arc::ptr_eq(a.arc(), b.arc()), "outside an execution too");
    assert_eq!(built.get(), 6);
    close(app).await;
}

#[tokio::test]
async fn singletons_reading_a_transient_each_hold_their_own() {
    let built = Count::new();
    let c = built.clone();
    let app = connected(Mod::<A>::new(move |m| {
        let c = c.clone();
        m.transient(move || {
            c.bump();
            async { Tr }
        });
        m.singleton(|tr: Dep<Tr>| async move { S1(tr) });
        m.singleton(|tr: Dep<Tr>| async move { S2(tr) });
    }))
    .await;
    assert_eq!(built.get(), 2, "one per reading site, built at connect");
    let (s1, s2) = (app.get::<S1>().await.unwrap(), app.get::<S2>().await.unwrap());
    assert!(!Arc::ptr_eq(s1.0.arc(), s2.0.arc()));
    close(app).await;
}

macro_rules! counting_controller {
    ($name:ident, $scope:ty, $built:ident, [$($field:ident : $ty:ty),*]) => {
        static $built: AtomicUsize = AtomicUsize::new(0);

        struct $name;

        impl ulo::Construct for $name {
            type Scope = $scope;

            fn dependencies(d: &mut ulo::Dependencies) {
                $( d.field::<$ty>(stringify!($field)); )*
                let _ = d;
            }

            async fn construct(r: &ulo::Resolver<'_>) -> Result<Self, ulo::ConstructError> {
                $( let _ = <$ty as ulo::FromContainer>::from_container(r).await?; )*
                let _ = r;
                $built.fetch_add(1, Ordering::SeqCst);
                Ok($name)
            }
        }

        impl ulo::Controller for $name {
            fn mount(m: &mut ulo::Mount<'_>) {
                m.handler::<TA, _>(HandlerSpec::new("h", ()));
            }
        }
    };
}

counting_controller!(ReadsExt, Auto, BUILT_READS_EXT, [user: Ext<User>]);
counting_controller!(ReadsAudit, Auto, BUILT_READS_AUDIT, [audit: Dep<Audit>]);
counting_controller!(ReadsNothing, Auto, BUILT_READS_NOTHING, []);
counting_controller!(ExplicitPerCall, PerExecution, BUILT_EXPLICIT, []);

struct Audit;

/// One call of the controller `C` through its mounted handler: opens an execution in the
/// handler's module, writes the `User` extension a guard would have written, and reads the
/// controller twice inside `dispatch`, as a handler does.
async fn call<C: ulo::Construct>(handler: &MountedHandler<TA>) {
    let exec = Execution::open(handler.module(), ExecOptions::new()).expect("the app is running");
    exec.extensions().insert(User);
    let calls = exec.handle();
    ulo::dispatch(handler, &calls, &(), |_cx| async {
        let a = calls.get::<C>().await?;
        let b = calls.get::<C>().await?;
        assert!(Arc::ptr_eq(a.arc(), b.arc()), "one controller per call");
        Ok(())
    })
    .await
    .expect("the call answered");
}

async fn mounted(app: &App<Connected>) -> MountedHandler<TA> {
    let mut handlers = app.handle().mounted::<TA>().expect("the app has a runtime");
    assert_eq!(handlers.len(), 1);
    handlers.remove(0)
}

#[tokio::test]
async fn a_controller_reading_execution_data_is_built_per_call() {
    let app = connected_on_tokio(Mod::<A>::new(|m| {
        m.controller::<ReadsExt>();
    }))
    .await;
    assert_eq!(BUILT_READS_EXT.load(Ordering::SeqCst), 0, "not built at connect");
    let handler = mounted(&app).await;
    for _ in 0..3 {
        call::<ReadsExt>(&handler).await;
    }
    assert_eq!(BUILT_READS_EXT.load(Ordering::SeqCst), 3, "one per call");
    drop(handler);
    close(app).await;
}

#[tokio::test]
async fn a_controller_reaching_execution_data_through_a_dependency_is_built_per_call() {
    let app = connected_on_tokio(Mod::<A>::new(|m| {
        m.execution(|_user: Ext<User>| async { Audit });
        m.controller::<ReadsAudit>();
    }))
    .await;
    let handler = mounted(&app).await;
    for _ in 0..3 {
        call::<ReadsAudit>(&handler).await;
    }
    assert_eq!(BUILT_READS_AUDIT.load(Ordering::SeqCst), 3);
    drop(handler);
    close(app).await;
}

#[tokio::test]
async fn an_auto_controller_that_reads_no_execution_data_is_built_once() {
    let app = connected_on_tokio(Mod::<A>::new(|m| {
        m.controller::<ReadsNothing>();
    }))
    .await;
    assert_eq!(BUILT_READS_NOTHING.load(Ordering::SeqCst), 1, "built at connect");
    let handler = mounted(&app).await;
    for _ in 0..3 {
        call::<ReadsNothing>(&handler).await;
    }
    assert_eq!(BUILT_READS_NOTHING.load(Ordering::SeqCst), 1, "calls build nothing");
    drop(handler);
    close(app).await;
}

#[tokio::test]
async fn an_explicitly_per_execution_controller_is_built_per_call_though_it_reads_nothing() {
    let app = connected_on_tokio(Mod::<A>::new(|m| {
        m.controller::<ExplicitPerCall>();
    }))
    .await;
    assert_eq!(BUILT_EXPLICIT.load(Ordering::SeqCst), 0);
    let handler = mounted(&app).await;
    for _ in 0..2 {
        call::<ExplicitPerCall>(&handler).await;
    }
    assert_eq!(BUILT_EXPLICIT.load(Ordering::SeqCst), 2);
    drop(handler);
    close(app).await;
}
