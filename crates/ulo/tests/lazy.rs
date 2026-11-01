//! Loading a module after startup (section 8.6).
//!
//! Capabilities pinned: 38 (a module is wired against the frozen graph and connected through its
//! own constructors, checks and init hooks; an equal identity is loaded once; a failed load leaves
//! the app as it found it; what the graph cannot take is refused with the `LoadRefusal` that names
//! it; a lazy module's hooks run first at shutdown, latest load first).
//!
//! `LoadError::Closed` from Stopping on, and after the shutdown, is pinned by the `load` lines of
//! `phases.rs`.

#[macro_use]
mod common;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use common::*;
use ulo::scope::Auto;
use ulo::{
    AppHandle, ConnectError, Dep, FailureReason, Key, LoadError, LoadRefusal, LookupError, LookupKind, Meta, Signal,
    WiringError,
};

struct A;
struct G;
struct L1;
struct L2;
struct Svc;
struct Cfg(u8);

async fn base_with(root: impl ulo::Module) -> (ulo::App<ulo::Connected>, AppHandle) {
    let app = connects(root).await;
    let handle = app.handle();
    (app, handle)
}

async fn refused(handle: &AppHandle, module: impl ulo::Module) -> LoadRefusal {
    match handle.load(module).await {
        Err(LoadError::Refused(refusal)) => refusal,
        Err(other) => panic!("expected a refusal, got: {other}"),
        Ok(_) => panic!("the load was not refused"),
    }
}

#[tokio::test]
async fn a_module_loaded_after_startup_is_built_checked_and_started_then_read_through_its_handle() {
    let log = Log::new();
    let built = Count::new();
    let (l, b) = (log.clone(), built.clone());
    let (app, handle) = base_with(Mod::<A>::new(|m| {
        m.import(Mod::<G>::new(|m| {
            m.global();
            m.value(Cfg(9));
            m.export::<Cfg>();
        }));
    }))
    .await;

    let lazy = Mod::<L1>::new(move |m| {
        let (built, checked, init) = (b.clone(), l.clone(), l.clone());
        // Reads a global export of the base graph, which a lazy module sees like any module.
        m.singleton(move |cfg: Dep<Cfg>| {
            built.bump();
            let n = cfg.0;
            async move { (Svc, n) }
        })
        .ready(move |_s: Dep<(Svc, u8)>| {
            let log = checked.clone();
            async move {
                log.push("checked");
                Ok::<(), Boom>(())
            }
        })
        .on_init(log_init(init));
    });
    assert_eq!(built.get(), 0, "nothing of the module exists before it is loaded");

    let module = handle.load(lazy).await.unwrap_or_else(|error| panic!("load failed: {error}"));
    assert_eq!(built.get(), 1, "the module's singleton was built by the load");
    assert_eq!(log.events(), ["checked", "init"], "its check and init hook ran during the load");
    assert_eq!(module.name().to_string(), mod_name::<L1>());
    let svc = module.get::<(Svc, u8)>().await.expect("the module reads its own binding");
    assert_eq!(svc.1, 9, "and the global export it was built from");
    assert!(
        matches!(app.get::<(Svc, u8)>().await, Err(LookupError::NotFound { kind: LookupKind::Binding, .. })),
        "the base graph's modules do not see a lazy module's bindings"
    );
    close(app).await;
}

fn log_init(log: Log) -> impl Fn() -> std::future::Ready<Result<(), Boom>> + Send + Sync + 'static {
    move || {
        log.push("init");
        std::future::ready(Ok(()))
    }
}

#[tokio::test]
async fn an_equal_identity_loads_once_and_a_module_the_graph_already_holds_returns_its_handle() {
    let built = Count::new();
    let (app, handle) = base_with(Mod::<A>::new(|m| {
        m.import(Mod::<G>::new(|m| {
            m.value(Cfg(1));
        }));
    }))
    .await;
    let make = |c: &Count| {
        let c = c.clone();
        Mod::<L1>::new(move |m| {
            let c = c.clone();
            m.singleton(move || {
                c.bump();
                async { Svc }
            });
        })
    };
    let first = handle.load(make(&built)).await.unwrap_or_else(|error| panic!("load failed: {error}"));
    let second = handle.load(make(&built)).await.unwrap_or_else(|error| panic!("load failed: {error}"));
    assert_eq!(built.get(), 1, "the second load of an equal identity built nothing");
    assert_eq!(first.name().to_string(), second.name().to_string());

    let held = handle.load(Mod::<G>::bare()).await.unwrap_or_else(|error| panic!("load failed: {error}"));
    assert_eq!(held.name().to_string(), mod_name::<G>(), "a module the base graph imports answers its existing handle");
    assert!(held.get::<Cfg>().await.is_ok(), "and is the base module, not an empty replacement");
    close(app).await;
}

#[tokio::test]
async fn concurrent_loads_are_serialised_so_a_failing_load_cannot_undo_another() {
    let (app, handle) = base_with(Empty).await;
    // `L1` yields inside its init hook and then fails, so its rollback comes after `L2`'s whole load
    // unless the loads wait for one another.
    let failing = Mod::<L1>::new(|m| {
        m.singleton(|| async { Svc }).on_init(|| async {
            for _ in 0..5 {
                tokio::task::yield_now().await;
            }
            Err::<(), _>(Boom::new("refused"))
        });
    });
    let healthy = Mod::<L2>::new(|m| {
        m.singleton(|| async { Cfg(4) });
    });
    let (failed, loaded) = tokio::join!(handle.load(failing), handle.load(healthy));
    assert!(matches!(failed, Err(LoadError::Connect(ConnectError::Hook { .. }))), "the first load fails in its init hook");
    let module = loaded.unwrap_or_else(|error| panic!("the second load failed: {error}"));
    assert!(handle.module::<Mod<L2>>().is_ok(), "the failed load's rollback took the other module out of the graph");
    assert_eq!(module.get::<Cfg>().await.expect("the loaded module reads its binding").0, 4);
    close(app).await;
}

#[derive(Default)]
struct Routes;
impl Meta for Routes {}
struct Addr;
struct Plugin;
trait Plug: Send + Sync {}
impl Plug for Plugin {}

controller!(LazyCtl, Auto, [], |_m| {});

#[tokio::test]
async fn a_module_with_a_controller_is_refused_and_builds_nothing() {
    let built = Count::new();
    let b = built.clone();
    let (app, handle) = base_with(Empty).await;
    let refusal = refused(
        &handle,
        Mod::<L1>::new(move |m| {
            let b = b.clone();
            m.singleton(move || {
                b.bump();
                async { Svc }
            });
            m.controller::<LazyCtl>();
        }),
    )
    .await;
    assert!(matches!(&refusal, LoadRefusal::Controllers { module } if module.to_string() == mod_name::<L1>()), "{refusal:?}");
    assert_eq!(built.get(), 0, "the refusal comes before anything is built");
    close(app).await;
}

#[tokio::test]
async fn a_module_that_writes_module_metadata_is_refused() {
    let (app, handle) = base_with(Empty).await;
    let refusal = refused(&handle, Mod::<L1>::new(|m| {
        m.meta::<Routes>();
    }))
    .await;
    assert!(matches!(&refusal, LoadRefusal::Middleware { module } if module.to_string() == mod_name::<L1>()), "{refusal:?}");
    close(app).await;
}

#[tokio::test]
async fn a_module_that_declares_an_execution_input_is_refused() {
    let (app, handle) = base_with(Empty).await;
    let refusal = refused(&handle, Mod::<L1>::new(|m| {
        m.input::<Addr>().seeded_by::<TA>();
    }))
    .await;
    let LoadRefusal::Input { module, key } = &refusal else { panic!("expected an input refusal, got: {refusal:?}") };
    assert_eq!((module.to_string(), key.key()), (mod_name::<L1>(), Key::of::<Addr, ()>()));
    close(app).await;
}

#[tokio::test]
async fn a_global_module_that_exports_is_refused_and_one_that_exports_nothing_loads() {
    let (app, handle) = base_with(Empty).await;
    let refusal = refused(&handle, Mod::<L1>::new(|m| {
        m.global();
        m.value(Cfg(1));
        m.export::<Cfg>();
    }))
    .await;
    let LoadRefusal::GlobalExport { module, key } = &refusal else { panic!("expected a global-export refusal, got: {refusal:?}") };
    assert_eq!((module.to_string(), key.key()), (mod_name::<L1>(), Key::of::<Cfg, ()>()));

    let loaded = handle
        .load(Mod::<L2>::new(|m| {
            m.global();
            m.value(Cfg(2));
        }))
        .await;
    assert!(loaded.is_ok(), "a global module with nothing exported leaves nothing to fix at wiring");
    close(app).await;
}

#[tokio::test]
async fn a_contribution_to_a_collection_the_graph_has_is_refused_and_one_that_introduces_it_loads() {
    let (app, handle) = base_with(Mod::<A>::new(|m| {
        m.contribute::<dyn Plug>().value(Arc::new(Plugin));
    }))
    .await;
    let refusal = refused(&handle, Mod::<L1>::new(|m| {
        m.contribute::<dyn Plug>().value(Arc::new(Plugin));
    }))
    .await;
    let LoadRefusal::Contribution { module, key } = &refusal else { panic!("expected a contribution refusal, got: {refusal:?}") };
    assert_eq!((module.to_string(), key.key()), (mod_name::<L1>(), Key::of::<dyn Plug, ()>()));

    trait Fresh: Send + Sync {}
    impl Fresh for Plugin {}
    let introduced = handle
        .load(Mod::<L2>::new(|m| {
            m.contribute::<dyn Fresh>().value(Arc::new(Plugin));
        }))
        .await;
    assert!(introduced.is_ok(), "a module may introduce a collection nothing reads yet");
    close(app).await;
}

#[tokio::test]
async fn a_module_with_a_wiring_fault_is_refused_with_its_errors_and_builds_nothing() {
    let built = Count::new();
    let b = built.clone();
    let (app, handle) = base_with(Empty).await;
    let error = handle
        .load(Mod::<L1>::new(move |m| {
            let b = b.clone();
            m.singleton(move || {
                b.bump();
                async { Svc }
            });
            m.singleton(|_c: Dep<Cfg>| async { Addr });
        }))
        .await;
    let Err(LoadError::Wiring(errors)) = error else { panic!("expected wiring errors") };
    let (key, module) = entry!(errors, WiringError::Missing { key, module, .. } => (key.key(), module.to_string()));
    assert_eq!((key, module), (Key::of::<Cfg, ()>(), mod_name::<L1>()));
    assert_eq!(built.get(), 0, "a module that does not wire builds nothing");
    assert!(handle.module::<Mod<L1>>().is_err(), "a module that did not wire is not in the graph");
    close(app).await;
}

struct Guard(Log, &'static str);

impl Drop for Guard {
    fn drop(&mut self) {
        self.0.push(self.1);
    }
}

#[tokio::test]
async fn a_load_that_fails_to_connect_leaves_the_app_as_found_and_a_retry_wires_again() {
    let log = Log::new();
    let built = Count::new();
    let fail = Arc::new(AtomicBool::new(true));
    let (app, handle) = base_with(Empty).await;
    let make = |log: &Log, built: &Count, fail: &Arc<AtomicBool>| {
        let (log, built, fail) = (log.clone(), built.clone(), Arc::clone(fail));
        Mod::<L1>::new(move |m| {
            let (log, built) = (log.clone(), built.clone());
            m.singleton(move || {
                built.bump();
                let guard = Guard(log.clone(), "svc dropped");
                async move { (Svc, guard) }
            });
            let fail = Arc::clone(&fail);
            m.try_singleton(move || {
                let fail = fail.load(Ordering::SeqCst);
                async move { if fail { Err(Boom::new("down")) } else { Ok(Cfg(1)) } }
            });
        })
    };

    let error = handle.load(make(&log, &built, &fail)).await.err().expect("the load succeeded");
    let LoadError::Connect(ConnectError::Construct { key, module, reason }) = error else { panic!("expected a construction failure") };
    assert_eq!(key.key(), Key::of::<Cfg, ()>());
    assert_eq!(module.to_string(), mod_name::<L1>(), "the module the failing binding is in, not the root");
    assert!(matches!(reason, FailureReason::Errored(_)));
    assert_eq!(built.get(), 1, "the singleton before the failure was built");
    assert!(log.has("svc dropped"), "and dropped again when the load failed");
    assert!(
        matches!(handle.module::<Mod<L1>>(), Err(LookupError::NotFound { kind: LookupKind::Module, .. })),
        "the module is not in the graph"
    );

    fail.store(false, Ordering::SeqCst);
    let module = handle.load(make(&log, &built, &fail)).await.unwrap_or_else(|error| panic!("the retry failed: {error}"));
    assert_eq!(built.get(), 2, "the retry wired and built the module again from the start");
    assert!(module.get::<Cfg>().await.is_ok());
    close(app).await;
}

#[tokio::test]
async fn a_failing_check_or_init_hook_fails_the_load_as_a_connect_error() {
    let (app, handle) = base_with(Empty).await;
    let readiness = handle
        .load(Mod::<L1>::new(|m| {
            m.singleton(|| async { Svc }).ready(|_s: Dep<Svc>| async { Err::<(), _>(Boom::new("down")) });
        }))
        .await
        .err()
        .expect("the load succeeded");
    assert!(matches!(readiness, LoadError::Connect(ConnectError::Readiness { attempts: 1, .. })), "got: {readiness}");

    let hook = handle
        .load(Mod::<L2>::new(|m| {
            m.on_init(|| async { Err::<(), _>(Boom::new("refused")) });
        }))
        .await
        .err()
        .expect("the load succeeded");
    assert!(matches!(hook, LoadError::Connect(ConnectError::Hook { .. })), "got: {hook}");
    close(app).await;
}

#[tokio::test]
async fn lazy_modules_shut_down_first_latest_load_first_then_the_base_graph() {
    let log = Log::new();
    let l = log.clone();
    let (app, handle) = base_with(Mod::<A>::new(move |m| {
        m.singleton(|| async { Cfg(0) })
            .before_shutdown(shutdown_hook(&l, "base before"))
            .on_destroy(destroy_hook(&l, "base destroy"))
            .on_shutdown(shutdown_hook(&l, "base on"));
    }))
    .await;
    let lazy = |name: &'static str, log: &Log| {
        let log = log.clone();
        move |m: &mut ulo::ModuleDef<'_>| {
            m.singleton(|| async { Svc })
                .before_shutdown(shutdown_hook(&log, leak(format!("{name} before"))))
                .on_destroy(destroy_hook(&log, leak(format!("{name} destroy"))))
                .on_shutdown(shutdown_hook(&log, leak(format!("{name} on"))));
        }
    };
    handle.load(Mod::<L1>::new(lazy("l1", &log))).await.unwrap();
    handle.load(Mod::<L2>::new(lazy("l2", &log))).await.unwrap();

    app.close(Signal::new("done")).await.expect("shutdown reported a failure");
    assert_eq!(
        log.events(),
        ["l2 before", "l1 before", "base before", "l2 destroy", "l1 destroy", "base destroy", "l2 on", "l1 on", "base on"],
        "in every hook step the lazily loaded modules come first, the latest load first"
    );
}

fn leak(text: String) -> &'static str {
    Box::leak(text.into_boxed_str())
}

fn shutdown_hook(log: &Log, text: &'static str) -> impl Fn(Signal) -> std::future::Ready<()> + Send + Sync + 'static {
    let log = log.clone();
    move |_s| {
        log.push(text);
        std::future::ready(())
    }
}

fn destroy_hook(log: &Log, text: &'static str) -> impl Fn() -> std::future::Ready<()> + Send + Sync + 'static {
    let log = log.clone();
    move || {
        log.push(text);
        std::future::ready(())
    }
}
