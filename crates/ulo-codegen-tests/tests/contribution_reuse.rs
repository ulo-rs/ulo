//! An `into K: [A]` item beside a providers entry binding `A` contributes that binding's
//! instance: `A` is built once and its init hook runs once, and the collection's item is the
//! object a direct read returns. An `into` item whose type the module binds nowhere else is a
//! binding of its own, built and hooked as one.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use ulo::{App, BoxError, Connected, ExecOptions, Many, OnModuleInit, injectable, module};

trait Plugin: Send + Sync {
    fn name(&self) -> &'static str;
}

trait Named: Send + Sync {}

/// A plugin counting its constructions and its init hook's runs in the two statics it names.
macro_rules! counted_plugin {
    ($ty:ident, $built:ident, $init:ident) => {
        static $built: AtomicUsize = AtomicUsize::new(0);
        static $init: AtomicUsize = AtomicUsize::new(0);

        struct $ty;

        #[injectable]
        impl $ty {
            fn new() -> Self {
                $built.fetch_add(1, Ordering::SeqCst);
                $ty
            }
        }

        impl Plugin for $ty {
            fn name(&self) -> &'static str {
                stringify!($ty)
            }
        }

        impl Named for $ty {}

        impl OnModuleInit for $ty {
            async fn on_module_init(&self) -> Result<(), BoxError> {
                $init.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        }
    };
}

counted_plugin!(Provided, PROVIDED_BUILT, PROVIDED_INIT);
counted_plugin!(ProvidedAs, PROVIDED_AS_BUILT, PROVIDED_AS_INIT);
counted_plugin!(Unprovided, UNPROVIDED_BUILT, UNPROVIDED_INIT);

#[module(providers = [Provided, ProvidedAs as dyn Named, into dyn Plugin: [Provided, ProvidedAs, Unprovided]])]
struct Plugins;

async fn connected(root: impl ulo::Module) -> App<Connected> {
    App::builder(root).wire().expect("wires").connect().await.expect("connects")
}

#[tokio::test]
async fn an_into_item_beside_its_providers_entry_is_that_binding_and_one_elsewhere_is_its_own() {
    let app = connected(Plugins).await;
    let all: Many<dyn Plugin> = app
        .execute(ExecOptions::new(), async |exec| exec.resolver().many::<dyn Plugin>().await)
        .await
        .expect("a connected app opens executions")
        .expect("the collection resolves");
    let names: Vec<&str> = all.iter().map(|plugin| plugin.name()).collect();
    assert_eq!(names, ["Provided", "ProvidedAs", "Unprovided"]);

    let provided = app.get::<Provided>().await.expect("`Provided` is bound");
    let provided_as = app.get::<ProvidedAs>().await.expect("`ProvidedAs` is bound");
    let items: Vec<&Arc<dyn Plugin>> = all.iter().collect();
    assert!(std::ptr::addr_eq(Arc::as_ptr(items[0]), Arc::as_ptr(provided.arc())), "the item is the provided instance");
    assert!(std::ptr::addr_eq(Arc::as_ptr(items[1]), Arc::as_ptr(provided_as.arc())), "the item is the instance `as dyn` binds");

    assert_eq!((PROVIDED_BUILT.load(Ordering::SeqCst), PROVIDED_INIT.load(Ordering::SeqCst)), (1, 1), "built once, hooked once");
    assert_eq!((PROVIDED_AS_BUILT.load(Ordering::SeqCst), PROVIDED_AS_INIT.load(Ordering::SeqCst)), (1, 1));
    assert_eq!(
        (UNPROVIDED_BUILT.load(Ordering::SeqCst), UNPROVIDED_INIT.load(Ordering::SeqCst)),
        (1, 1),
        "a type bound nowhere else is built for the collection, and its hook runs"
    );
}

static PER_CALL_BUILT: AtomicUsize = AtomicUsize::new(0);

struct PerCall;

#[injectable(execution)]
impl PerCall {
    fn new() -> Self {
        PER_CALL_BUILT.fetch_add(1, Ordering::SeqCst);
        PerCall
    }
}

impl Plugin for PerCall {
    fn name(&self) -> &'static str {
        "PerCall"
    }
}

#[module(providers = [PerCall, into dyn Plugin: [PerCall]])]
struct PerCallPlugins;

/// The reused binding keeps its own scope: an execution-scoped `A` is contributed per execution,
/// the item and a direct read in one execution are one instance, and each execution builds one.
#[tokio::test]
async fn a_reused_execution_scoped_binding_is_one_instance_per_execution() {
    let app = connected(PerCallPlugins).await;
    for _ in 0..2 {
        app.execute(ExecOptions::new(), async |exec| {
            let all = exec.resolver().many::<dyn Plugin>().await.expect("the collection resolves");
            let direct = exec.get::<PerCall>().await.expect("`PerCall` is bound");
            let item = all.iter().next().expect("one item");
            assert!(std::ptr::addr_eq(Arc::as_ptr(item), Arc::as_ptr(direct.arc())), "the item is the execution's instance");
        })
        .await
        .expect("a connected app opens executions");
    }
    assert_eq!(PER_CALL_BUILT.load(Ordering::SeqCst), 2, "one build per execution");
}
