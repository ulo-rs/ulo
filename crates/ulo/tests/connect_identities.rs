//! Four identities a lifecycle failure or read carries, pinned where an earlier suite asserted
//! only the variant (tenth response D3):
//!
//! - a hook resolves its parameters with the visibility of the module that declared it, not the
//!   root's;
//! - a keyed module's hook failure names the module with its qualifier;
//! - a construction that fails inside another construction names the module of the failing
//!   dependency, not the dependent's;
//! - `ExecutionRequired` names the key that needed the execution, at an `Ext`, an `Option<Ext>`,
//!   an `ExecutionRef` and an input site alike.

#[macro_use]
mod common;

use common::*;
use ulo::scope::Transient;
use ulo::{
    ConnectError, Construct, ConstructError, Dep, Dependencies, Ext, ExecutionRef, FailureReason, FromContainer, HookKind, Key,
    LookupError, Module, Resolver, StartupError,
};

struct Root;
struct M;
struct N;
struct K;
struct Primary;
struct Replica;

struct Hidden(u32);
struct Svc;

#[tokio::test]
async fn a_binding_s_and_a_module_s_hook_read_with_their_own_module_s_visibility() {
    let log = Log::new();
    let l = log.clone();
    let app = connects(Mod::<Root>::new(move |m| {
        // The root binds a `Hidden` of its own. Under the root's visibility a hook of `M` would
        // read this one; the module's own is `7`.
        m.value(Hidden(99));
        let log = l.clone();
        m.import(Mod::<M>::new(move |m| {
            m.value(Hidden(7));
            let (init, destroy, module_init, module_destroy) = (log.clone(), log.clone(), log.clone(), log.clone());
            m.singleton(|| async { Svc })
                .on_init(move |hidden: Dep<Hidden>| {
                    let log = init.clone();
                    async move {
                        log.push(format!("binding init saw {}", hidden.0));
                        Ok::<(), Boom>(())
                    }
                })
                .on_destroy(move |hidden: Dep<Hidden>| {
                    let log = destroy.clone();
                    async move { log.push(format!("binding destroy saw {}", hidden.0)) }
                });
            m.export::<Svc>();
            m.on_init(move |hidden: Dep<Hidden>| {
                let log = module_init.clone();
                async move {
                    log.push(format!("module init saw {}", hidden.0));
                    Ok::<(), Boom>(())
                }
            });
            m.on_destroy(move |hidden: Dep<Hidden>| {
                let log = module_destroy.clone();
                async move { log.push(format!("module destroy saw {}", hidden.0)) }
            });
        }));
    }))
    .await;

    // The root cannot read what `M` keeps to itself, and `M` reads its own.
    assert_eq!(app.get::<Hidden>().await.map(|hidden| hidden.0).ok(), Some(99));
    assert_eq!(app.module::<Mod<M>>().unwrap().get::<Hidden>().await.map(|hidden| hidden.0).ok(), Some(7));
    let startup: Vec<String> = log.events().into_iter().filter(|line| line.contains("init saw")).collect();
    assert_eq!(startup, ["binding init saw 7", "module init saw 7"]);

    close(app).await;
    let teardown: Vec<String> = log.events().into_iter().filter(|line| line.contains("destroy saw")).collect();
    assert_eq!(teardown.len(), 2, "{teardown:?}");
    assert!(teardown.contains(&"binding destroy saw 7".to_owned()), "{teardown:?}");
    assert!(teardown.contains(&"module destroy saw 7".to_owned()), "{teardown:?}");
}

#[tokio::test]
async fn a_keyed_module_s_failing_hook_names_the_module_with_its_qualifier() {
    let error = connect_err(Mod::<Root>::new(|m| {
        m.import(
            Mod::<K>::new(|m| {
                m.on_init(|| async { Ok::<(), Boom>(()) });
            })
            .keyed::<Primary>(),
        );
        m.import(
            Mod::<K>::new(|m| {
                m.on_init(|| async { Err::<(), _>(Boom::new("replica refused")) });
            })
            .keyed::<Replica>(),
        );
    }))
    .await;

    let StartupError::Connect(ConnectError::Hook { hook, key, reason }) = error else {
        panic!("expected a hook failure, got: {error}");
    };
    assert_eq!(hook, HookKind::OnModuleInit);
    assert_eq!(key.key(), Key::of::<Mod<K>, Replica>(), "the module's own type and the qualifier of the instance that failed");
    assert!(matches!(reason, FailureReason::Errored(_)), "got: {reason}");
}

struct Top;
struct Mid;
struct Deep;

/// `module` and `key` of the construction failure `connect` answers, and the error the deepest
/// constructor returned.
fn construction_failure(error: StartupError) -> (Key, String, String) {
    let StartupError::Connect(ConnectError::Construct { key, module, reason }) = error else {
        panic!("expected a construction failure, got: {error}");
    };
    let FailureReason::Errored(source) = reason else { panic!("expected an errored construction, got: {reason}") };
    (key.key(), module.to_string(), source.to_string())
}

#[tokio::test]
async fn a_construction_failing_inside_another_names_the_failing_dependency_s_module() {
    // `Deep` is transient, so `Top` builds it from inside its own constructor.
    let error = connect_err(Mod::<Root>::new(|m| {
        m.import(Mod::<N>::new(|m| {
            m.try_transient(|| async { Err::<Deep, _>(Boom::new("deep refused")) });
            m.export::<Deep>();
        }));
        m.singleton(|_deep: Dep<Deep>| async { Top });
    }))
    .await;

    let (key, module, message) = construction_failure(error);
    assert_eq!((key, module), (Key::of::<Deep, ()>(), mod_name::<N>()), "not `Top` and not the root that holds it");
    assert!(message.contains("deep refused"), "{message}");
}

#[tokio::test]
async fn a_failing_dependency_the_dependent_s_module_cannot_see_still_names_its_own_module() {
    // `Top` is in the root, which imports `M` and not `N`; `Mid` (exported by `M`) reads `Deep`
    // (exported by `N`). The root has no `Deep` in its visibility to look the module up by.
    let error = connect_err(Mod::<Root>::new(|m| {
        m.import(Mod::<M>::new(|m| {
            m.import(Mod::<N>::new(|m| {
                m.try_transient(|| async { Err::<Deep, _>(Boom::new("deep refused")) });
                m.export::<Deep>();
            }));
            m.transient(|_deep: Dep<Deep>| async { Mid });
            m.export::<Mid>();
        }));
        m.singleton(|_mid: Dep<Mid>| async { Top });
    }))
    .await;

    let (key, module, _) = construction_failure(error);
    assert_eq!((key, module), (Key::of::<Deep, ()>(), mod_name::<N>()));
}

struct User;

/// A transient whose constructor reads `$declares` from a resolver with no execution behind it:
/// declared, as the read must be, and built by `App::get` outside any execution. A singleton
/// declaring the read would not wire.
macro_rules! reads_without_an_execution {
    ($name:ident, $declares:ty, |$r:ident| $read:expr) => {
        struct $name;

        impl Construct for $name {
            type Scope = Transient;

            fn dependencies(d: &mut Dependencies) {
                d.add::<$declares>();
            }

            async fn construct($r: &Resolver<'_>) -> Result<Self, ConstructError> {
                let _ = $read.await?;
                Ok($name)
            }
        }
    };
}

reads_without_an_execution!(ReadsExt, Ext<User>, |r| async { Ext::<User>::from_container(r).await });
reads_without_an_execution!(ReadsOptionalExt, Option<Ext<User>>, |r| async { <Option<Ext<User>>>::from_container(r).await });
reads_without_an_execution!(ReadsExecution, ExecutionRef, |r| async { ExecutionRef::from_container(r).await });
reads_without_an_execution!(ReadsInput, Dep<User>, |r| async { r.input::<User>() });

async fn execution_required<S: Construct>() -> Key {
    let app = connects(Mod::<Root>::new(|m| {
        // Satisfies the `Dep<User>` the input site declares; the read itself is `r.input`.
        m.singleton(|| async { User });
        m.provide::<S>();
    }))
    .await;
    match app.get::<S>().await {
        Err(LookupError::ExecutionRequired { key }) => key.key(),
        Err(other) => panic!("expected `ExecutionRequired`, got: {other}"),
        Ok(_) => panic!("the build succeeded outside an execution"),
    }
}

#[tokio::test]
async fn execution_required_names_the_extension_type_at_an_ext_site() {
    assert_eq!(execution_required::<ReadsExt>().await, Key::of::<User, ()>());
}

#[tokio::test]
async fn execution_required_propagates_through_option_with_the_same_key() {
    assert_eq!(execution_required::<ReadsOptionalExt>().await, Key::of::<User, ()>());
}

#[tokio::test]
async fn execution_required_names_execution_ref_at_an_execution_site() {
    assert_eq!(execution_required::<ReadsExecution>().await, Key::of::<ExecutionRef, ()>());
}

#[tokio::test]
async fn execution_required_names_the_input_type_at_an_input_site() {
    assert_eq!(execution_required::<ReadsInput>().await, Key::of::<User, ()>());
}
