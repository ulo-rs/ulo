//! `ModuleRef`: the handle to one module, whose lookups are limited to what that module sees.
//!
//! Capabilities pinned (CAPABILITIES.md ids): 37 (a handle to the current or a named module for
//! runtime lookups limited to what the module sees), with the typed errors of capability 47 that
//! such a lookup can answer (`NotFound`, `Ambiguous`, `AmbiguousModule`, `ExecutionRequired`).
//! The full `LookupError` table belongs to `lookup.rs`; `NotReady` during `connect` is B3's.

#[macro_use]
mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use common::*;
use ulo::scope::{PerExecution, Transient};
use ulo::{
    Construct, ConstructError, Dep, Dependencies, ExecOptions, ExecutionRef, Key, LookupError, LookupKind, Meta, Module, ModuleDef, ModuleIdentity,
    ModuleRef, Resolver,
};

struct A;
struct B;
struct C;
struct D;
struct Absent;

struct Svc;
struct Hidden;
struct Other;
struct PerCall;

#[tokio::test]
async fn a_module_ref_sees_only_what_its_module_sees() {
    let app = connects(Mod::<A>::new(|m| {
        m.import(Mod::<B>::new(|m| {
            m.singleton(|| async { Hidden });
            m.singleton(|| async { Svc });
            m.export::<Svc>();
        }));
        m.import(Mod::<C>::new(|m| {
            m.singleton(|| async { Other });
            m.export::<Other>();
        }));
    }))
    .await;

    let b = app.module::<Mod<B>>().unwrap();
    assert!(b.get::<Hidden>().await.is_ok(), "a module sees its own bindings");
    assert!(b.get::<Svc>().await.is_ok());
    assert!(
        matches!(b.get::<Other>().await, Err(LookupError::NotFound { kind: LookupKind::Binding, .. })),
        "`B` does not import `C`"
    );

    // The root sees `B`'s export and `C`'s, not `B`'s private binding; `C` sees none of `B`.
    assert!(matches!(app.get::<Hidden>().await, Err(LookupError::NotFound { .. })));
    assert!(app.get::<Svc>().await.is_ok());
    let c = app.module::<Mod<C>>().unwrap();
    assert!(matches!(c.get::<Svc>().await, Err(LookupError::NotFound { .. })));
}

#[tokio::test]
async fn a_module_ref_reports_a_key_its_module_sees_twice() {
    // Nothing in `B` reads `Svc`, so wiring accepts the two sources; a runtime lookup meets them.
    let app = connects(Mod::<A>::new(|m| {
        m.import(Mod::<B>::new(|m| {
            m.import(Mod::<C>::new(|m| {
                m.singleton(|| async { Svc });
                m.export::<Svc>();
            }));
            m.import(Mod::<D>::new(|m| {
                m.singleton(|| async { Svc });
                m.export::<Svc>();
            }));
        }));
    }))
    .await;

    let b = app.module::<Mod<B>>().unwrap();
    let Err(LookupError::Ambiguous { key, sources }) = b.get::<Svc>().await else {
        panic!("`B` sees `Svc` from two imports");
    };
    assert_eq!(key.key(), ulo::Key::of::<Svc, ()>());
    let sources: Vec<String> = sources.iter().map(|s| s.to_string()).collect();
    assert_eq!(sources, [mod_name::<C>(), mod_name::<D>()]);
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct Configured(u32);

impl Module for Configured {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_value(self)
    }

    fn register(&self, _m: &mut ModuleDef<'_>) {}
}

#[tokio::test]
async fn a_module_lookup_by_type_is_not_found_or_ambiguous() {
    let app = connects(Mod::<A>::new(|m| {
        m.import(Configured(1));
        m.import(Configured(2));
        m.import(Mod::<B>::bare());
    }))
    .await;

    let Err(LookupError::AmbiguousModule { candidates, .. }) = app.module::<Configured>() else {
        panic!("two configurations of `Configured` are not one module");
    };
    assert_eq!(candidates.len(), 2);
    assert!(matches!(app.module::<Mod<Absent>>(), Err(LookupError::NotFound { kind: LookupKind::Module, .. })));
    assert!(app.module::<Mod<B>>().is_ok());
}

#[tokio::test]
async fn a_module_ref_inside_an_execution_builds_per_execution_bindings_once() {
    let app = connects(Mod::<A>::new(|m| {
        m.import(Mod::<B>::new(|m| {
            m.execution(|| async { PerCall });
        }));
    }))
    .await;

    let b = app.module::<Mod<B>>().unwrap();
    assert!(
        matches!(b.get::<PerCall>().await, Err(LookupError::ExecutionRequired { .. })),
        "outside an execution a per-execution binding has nowhere to live"
    );

    let (first, second, inside_again) = app
        .execute(ExecOptions::new(), async |exec| {
            let here = b.with_execution(&exec.handle());
            let first = here.get::<PerCall>().await.unwrap();
            let second = here.get::<PerCall>().await.unwrap();
            // A second handle for the same execution reads the same cache.
            let again = b.with_execution(&exec.handle()).get::<PerCall>().await.unwrap();
            (first, second, again)
        })
        .await
        .unwrap();
    assert!(Arc::ptr_eq(first.arc(), second.arc()));
    assert!(Arc::ptr_eq(first.arc(), inside_again.arc()));

    // Another execution builds its own.
    let other = app
        .execute(ExecOptions::new(), async |exec| b.with_execution(&exec.handle()).get::<PerCall>().await.unwrap())
        .await
        .unwrap();
    assert!(!Arc::ptr_eq(first.arc(), other.arc()));
}

#[tokio::test]
async fn execute_on_a_module_ref_resolves_with_that_module_s_visibility() {
    let app = connects(Mod::<A>::new(|m| {
        m.import(Mod::<B>::new(|m| {
            m.singleton(|| async { Hidden });
        }));
    }))
    .await;

    let from_b = app.module::<Mod<B>>().unwrap().execute(ExecOptions::new(), async |exec| exec.get::<Hidden>().await.is_ok()).await.unwrap();
    let from_root = app.execute(ExecOptions::new(), async |exec| exec.get::<Hidden>().await.is_ok()).await.unwrap();
    assert!(from_b);
    assert!(!from_root);
}

#[derive(Default)]
struct Routes {
    names: Vec<&'static str>,
}

impl Meta for Routes {}

#[tokio::test]
async fn meta_is_what_the_module_wrote_and_nothing_for_another_module() {
    let app = connects(Mod::<A>::new(|m| {
        m.import(Mod::<B>::new(|m| {
            m.meta::<Routes>().names.push("health");
            m.meta::<Routes>().names.push("metrics");
        }));
        m.import(Mod::<C>::bare());
    }))
    .await;

    let written = app.module::<Mod<B>>().unwrap().meta::<Routes>().expect("`B` wrote `Routes`");
    assert_eq!(written.names, ["health", "metrics"], "both writes land in the one value");
    assert!(app.module::<Mod<C>>().unwrap().meta::<Routes>().is_none());
}

struct Holder(ModuleRef);

#[tokio::test]
async fn an_injected_module_ref_is_the_enclosing_module() {
    let app = connects(Mod::<A>::new(|m| {
        m.import(Mod::<B>::new(|m| {
            m.singleton(|| async { Hidden });
            m.singleton(|here: ModuleRef| async move { Holder(here) });
            m.export::<Holder>();
        }));
    }))
    .await;

    let holder = app.get::<Holder>().await.unwrap();
    assert_eq!(holder.0.name().to_string(), mod_name::<B>());
    assert!(holder.0.get::<Hidden>().await.is_ok(), "the handle reads what `B` sees, a private binding included");
    assert!(app.get::<Hidden>().await.is_err());
}

/// A construction that reads a handle, its `ModuleRef` or its `ExecutionRef` as `$handle` takes
/// it, and through it `Back`, whose construction reads the reader back: a cycle `wire()` cannot
/// see, since neither handle declares a key. `$depth` counts the reader's constructions, so a
/// recursion the refusal fails to stop ends in an error before it can overflow the stack.
macro_rules! reads_back {
    ($reader:ident, $back:ident, $scope:ty, $depth:ident, $handle:expr) => {
        static $depth: AtomicUsize = AtomicUsize::new(0);

        struct $reader;

        impl Construct for $reader {
            type Scope = $scope;

            fn dependencies(d: &mut Dependencies) {
                d.field::<ModuleRef>("here");
                d.field::<ExecutionRef>("exec");
            }

            async fn construct(r: &Resolver<'_>) -> Result<Self, ConstructError> {
                if $depth.fetch_add(1, Ordering::SeqCst) >= 16 {
                    return Err(ConstructError::Failed(Box::new(Boom::new("the construction recursed 16 deep"))));
                }
                ($handle)(r).get::<$back>().await?;
                Ok($reader)
            }
        }

        struct $back;

        impl Construct for $back {
            type Scope = $scope;

            fn dependencies(d: &mut Dependencies) {
                d.field::<Dep<$reader>>("reader");
            }

            async fn construct(r: &Resolver<'_>) -> Result<Self, ConstructError> {
                r.dep::<$reader>().await?;
                Ok($back)
            }
        }
    };
}

reads_back!(ExecReader, ExecBack, PerExecution, EXEC_DEPTH, |r: &Resolver<'_>| r.module());
reads_back!(TransientReader, TransientBack, Transient, TRANSIENT_DEPTH, |r: &Resolver<'_>| r.module());
reads_back!(HandleReader, HandleBack, PerExecution, HANDLE_DEPTH, |r: &Resolver<'_>| r.execution().unwrap());

/// The key read while it was being built and the key of the construction that read it back, from
/// a `Reentrant` refusal.
fn reentrant_keys<T: ?Sized>(answer: Result<Dep<T>, LookupError>) -> (Key, Key) {
    match answer {
        Err(LookupError::Reentrant { key, reader }) => (key.key(), reader.key()),
        Err(other) => panic!("expected `Reentrant`, got: {other}"),
        Ok(_) => panic!("expected `Reentrant`, the read succeeded"),
    }
}

#[tokio::test]
async fn an_execution_scoped_construction_read_back_through_its_module_ref_is_refused_naming_both_keys() {
    let app = connects(Mod::<A>::new(|m| {
        m.provide::<ExecReader>();
        m.provide::<ExecBack>();
    }))
    .await;
    let answer = app
        .execute(ExecOptions::new(), async |exec| within("a read of a construction that reads itself back", exec.get::<ExecReader>()).await)
        .await
        .unwrap();
    let (key, reader) = reentrant_keys(answer);
    assert_eq!(key, Key::of::<ExecReader, ()>(), "the binding read while it was being built");
    assert_eq!(reader, Key::of::<ExecBack, ()>(), "the construction that read it back");
    assert_eq!(EXEC_DEPTH.load(Ordering::SeqCst), 1, "the reader was built once");
}

/// Run on a thread with a stack far larger than the depth counter's bound needs, so a recursion
/// the refusal fails to stop ends in the counter's error rather than aborting the test process.
#[test]
fn a_transient_construction_read_back_through_its_module_ref_is_refused_rather_than_recursing() {
    let run = || {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        runtime.block_on(async {
            let app = connects(Mod::<A>::new(|m| {
                m.provide::<TransientReader>();
                m.provide::<TransientBack>();
            }))
            .await;
            app.execute(ExecOptions::new(), async |exec| exec.get::<TransientReader>().await).await.unwrap()
        })
    };
    let answer = std::thread::Builder::new().stack_size(128 << 20).spawn(run).unwrap().join().unwrap();
    let (key, reader) = reentrant_keys(answer);
    assert_eq!(key, Key::of::<TransientReader, ()>());
    assert_eq!(reader, Key::of::<TransientBack, ()>());
    assert_eq!(TRANSIENT_DEPTH.load(Ordering::SeqCst), 1, "no construction recursed");
}

#[tokio::test]
async fn a_construction_read_back_through_its_execution_ref_is_refused_naming_both_keys() {
    let app = connects(Mod::<A>::new(|m| {
        m.provide::<HandleReader>();
        m.provide::<HandleBack>();
    }))
    .await;
    let answer = app
        .execute(ExecOptions::new(), async |exec| within("a read of a construction that reads itself back", exec.get::<HandleReader>()).await)
        .await
        .unwrap();
    let (key, reader) = reentrant_keys(answer);
    assert_eq!(key, Key::of::<HandleReader, ()>());
    assert_eq!(reader, Key::of::<HandleBack, ()>());
}

struct KeepsHandle(ModuleRef);
struct LaterRead;

#[tokio::test]
async fn a_module_ref_kept_past_its_construction_reads_that_binding_again() {
    let app = connects(Mod::<A>::new(|m| {
        m.transient(|here: ModuleRef| async move { KeepsHandle(here) });
        m.transient(|| async { LaterRead });
    }))
    .await;
    app.execute(ExecOptions::new(), async |exec| {
        let kept = exec.get::<KeepsHandle>().await.unwrap();
        assert!(kept.0.get::<KeepsHandle>().await.is_ok(), "a construction that has ended refuses nothing");
        assert!(kept.0.get::<LaterRead>().await.is_ok());
    })
    .await
    .unwrap();
}

/// A construction that opens a new execution through its `ModuleRef`, by `$open`, and reads
/// `Back` there, whose construction reads the reader back: each step of the cycle runs in an
/// execution of its own, so neither the cache nor the handle a step reads carries the earlier
/// steps unless the execution does. `$depth` bounds the recursion as `reads_back!`'s does.
macro_rules! reads_back_in_a_new_execution {
    ($reader:ident, $back:ident, $scope:ty, $depth:ident, $open:expr) => {
        static $depth: AtomicUsize = AtomicUsize::new(0);

        struct $reader;

        impl Construct for $reader {
            type Scope = $scope;

            fn dependencies(d: &mut Dependencies) {
                d.field::<ModuleRef>("here");
            }

            async fn construct(r: &Resolver<'_>) -> Result<Self, ConstructError> {
                if $depth.fetch_add(1, Ordering::SeqCst) >= 16 {
                    return Err(ConstructError::Failed(Box::new(Boom::new("the construction recursed 16 deep"))));
                }
                let answer: Result<Dep<$back>, LookupError> = ($open)(r.module()).await;
                answer?;
                Ok($reader)
            }
        }

        struct $back;

        impl Construct for $back {
            type Scope = $scope;

            fn dependencies(d: &mut Dependencies) {
                d.field::<Dep<$reader>>("reader");
            }

            async fn construct(r: &Resolver<'_>) -> Result<Self, ConstructError> {
                r.dep::<$reader>().await?;
                Ok($back)
            }
        }
    };
}

reads_back_in_a_new_execution!(ExecuteTransient, ExecuteTransientBack, Transient, EXECUTE_TRANSIENT_DEPTH, |here: ModuleRef| async move {
    here.execute(ExecOptions::new(), async |exec| exec.get::<ExecuteTransientBack>().await).await.expect("the execution opens")
});
reads_back_in_a_new_execution!(ExecutePerCall, ExecutePerCallBack, PerExecution, EXECUTE_PER_CALL_DEPTH, |here: ModuleRef| async move {
    here.execute(ExecOptions::new(), async |exec| exec.handle().get::<ExecutePerCallBack>().await).await.expect("the execution opens")
});
reads_back_in_a_new_execution!(OpenPerCall, OpenPerCallBack, PerExecution, OPEN_PER_CALL_DEPTH, |here: ModuleRef| async move {
    let exec = ulo::Execution::open(&here, ExecOptions::new()).expect("the execution opens");
    exec.get::<OpenPerCallBack>().await
});

/// `f` on a thread with a stack far larger than the depth counter's bound needs, so a recursion
/// the refusal fails to stop ends in the counter's error rather than aborting the test process.
fn on_a_large_stack<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::Builder::new().stack_size(128 << 20).spawn(f).unwrap().join().unwrap()
}

/// What reading `Reader` in a standalone execution of an app providing `Reader` and `Back` answers.
fn read_in_an_execution<Reader: Construct + Send + Sync + 'static, Back: Construct + Send + Sync + 'static>() -> Result<Dep<Reader>, LookupError> {
    on_a_large_stack(|| {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        runtime.block_on(async {
            let app = connects(Mod::<A>::new(|m| {
                m.provide::<Reader>();
                m.provide::<Back>();
            }))
            .await;
            app.execute(ExecOptions::new(), async |exec| within("a read of a construction that reads itself back", exec.get::<Reader>()).await)
                .await
                .unwrap()
        })
    })
}

#[test]
fn a_transient_construction_read_back_through_an_execution_its_module_ref_opens_is_refused() {
    let (key, reader) = reentrant_keys(read_in_an_execution::<ExecuteTransient, ExecuteTransientBack>());
    assert_eq!(key, Key::of::<ExecuteTransient, ()>());
    assert_eq!(reader, Key::of::<ExecuteTransientBack, ()>());
    assert_eq!(EXECUTE_TRANSIENT_DEPTH.load(Ordering::SeqCst), 1, "no construction recursed");
}

/// The execution `execute` opens has a cache of its own, where the reader is not under way; the
/// chain it carries is what refuses the read, through a handle `Execution::handle` gives.
#[test]
fn an_execution_scoped_construction_read_back_through_an_execution_its_module_ref_opens_is_refused() {
    let (key, reader) = reentrant_keys(read_in_an_execution::<ExecutePerCall, ExecutePerCallBack>());
    assert_eq!(key, Key::of::<ExecutePerCall, ()>());
    assert_eq!(reader, Key::of::<ExecutePerCallBack, ()>());
    assert_eq!(EXECUTE_PER_CALL_DEPTH.load(Ordering::SeqCst), 1, "no construction recursed");
}

#[test]
fn a_construction_read_back_through_an_execution_opened_on_its_module_ref_is_refused() {
    let (key, reader) = reentrant_keys(read_in_an_execution::<OpenPerCall, OpenPerCallBack>());
    assert_eq!(key, Key::of::<OpenPerCall, ()>());
    assert_eq!(reader, Key::of::<OpenPerCallBack, ()>());
    assert_eq!(OPEN_PER_CALL_DEPTH.load(Ordering::SeqCst), 1, "no construction recursed");
}

struct KeepsHandleToExecute(ModuleRef);

/// A handle kept past its construction carries a chain that has ended: an execution opened through
/// it builds the binding the construction was for.
#[tokio::test]
async fn an_execution_opened_through_a_module_ref_kept_past_its_construction_reads_that_binding_again() {
    let app = connects(Mod::<A>::new(|m| {
        m.transient(|here: ModuleRef| async move { KeepsHandleToExecute(here) });
    }))
    .await;
    let kept = app.execute(ExecOptions::new(), async |exec| exec.get::<KeepsHandleToExecute>().await.unwrap()).await.unwrap();
    let again = kept.0.execute(ExecOptions::new(), async |exec| exec.get::<KeepsHandleToExecute>().await.is_ok()).await.unwrap();
    assert!(again, "a construction that has ended refuses nothing");
}
