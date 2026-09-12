//! Startup hooks fire in one order: a module's `on_module_init` before its
//! providers', and every `on_module_init` before any `on_application_bootstrap`.
//!
//! `on_module_init` runs during `UloFactory::create()` and
//! `on_application_bootstrap` during `app.bind()`, which is the split a
//! provider opening a connection depends on — it is ready by the time anything
//! bootstraps against it. Shutdown is covered from both ends, since a teardown
//! hook that runs twice is as wrong as one that never runs.

use std::sync::{Arc, Mutex, OnceLock};

use crate::common::NotServed;
use serial_test::serial;
use ulo::extract::Payload;
use ulo::grpc::extract::Inbound;
use ulo::{UloFactory, injectable, module, on_application_bootstrap, on_module_init};
use ulo_http_axum::AxumAdapter;
use ulo_macros::{controller, on_application_shutdown, on_module_destroy, patterns, routes};
use ulo_rpc_tcp::TcpAdapter;

static EVENT_LOG: OnceLock<Arc<Mutex<Vec<&'static str>>>> = OnceLock::new();

fn get_log() -> Arc<Mutex<Vec<&'static str>>> {
    EVENT_LOG
        .get_or_init(|| Arc::new(Mutex::new(Vec::new())))
        .clone()
}

#[injectable]
pub struct HookedService {}
impl HookedService {
    #[on_module_init]
    async fn on_module_init(&self) -> ulo::InitResult {
        get_log().lock().unwrap().push("provider:init");
        Ok(())
    }

    #[on_application_bootstrap]
    async fn on_application_bootstrap(&self) -> ulo::InitResult {
        get_log().lock().unwrap().push("provider:bootstrap");
        Ok(())
    }
}

#[module(providers: [HookedService])]
impl HookModule {
    #[on_module_init]
    async fn on_module_init(&self) -> ulo::InitResult {
        get_log().lock().unwrap().push("module:init");
        Ok(())
    }

    #[on_application_bootstrap]
    async fn on_module_bootstrap(&self) -> ulo::InitResult {
        get_log().lock().unwrap().push("module:bootstrap");
        Ok(())
    }
}

#[serial]
#[tokio_localset_test::localset_test]
async fn startup_hooks_fire_in_order() {
    get_log().lock().unwrap().clear();

    let mut app = UloFactory::create(HookModule).await.unwrap();
    app.use_http_adapter(AxumAdapter::new(), ("127.0.0.1", 0))
        .unwrap();
    app.bind().await.unwrap();

    let log = get_log().lock().unwrap().clone();
    assert_eq!(
        log,
        vec![
            "module:init",
            "provider:init",
            "module:bootstrap",
            "provider:bootstrap",
        ],
        "expected module init → provider init → module bootstrap → provider bootstrap"
    );
}

// Module-impl hooks are collected by an attribute scan (provider hooks expand through
// the standalone macros, which resolve by path on their own). The scan must accept the
// path-qualified spelling too.
#[tokio_localset_test::localset_test]
async fn path_qualified_module_hook_attr_fires() {
    static LOG: OnceLock<Arc<Mutex<Vec<&'static str>>>> = OnceLock::new();
    fn qualified_log() -> Arc<Mutex<Vec<&'static str>>> {
        LOG.get_or_init(|| Arc::new(Mutex::new(Vec::new()))).clone()
    }

    #[module(providers: [])]
    impl QualifiedHookModule {
        #[ulo::on_module_init]
        async fn on_module_init(&self) -> ulo::InitResult {
            qualified_log().lock().unwrap().push("module:init");
            Ok(())
        }
    }

    let _app = UloFactory::create(QualifiedHookModule).await.unwrap();
    assert_eq!(qualified_log().lock().unwrap().clone(), vec!["module:init"]);
}

#[controller]
pub struct HookedRpcController {}

#[patterns]
impl HookedRpcController {
    #[on_module_init]
    async fn ready(&self) -> ulo::InitResult {
        get_log().lock().unwrap().push("rpc-controller:init");
        Ok(())
    }

    #[on_application_bootstrap]
    async fn started(&self) -> ulo::InitResult {
        get_log().lock().unwrap().push("rpc-controller:bootstrap");
        Ok(())
    }
}

#[module(controllers: [HookedRpcController])]
impl RpcHookModule {}

/// An RPC controller is kept out of the module's provider map so nothing can inject it, and the
/// startup hooks still reach it. The map excluding it is the same one the hook loops read, so
/// dropping it there without a second home would have silenced these hooks and nothing else.
#[serial]
#[tokio_localset_test::localset_test]
async fn an_rpc_controller_still_gets_its_startup_hooks() {
    get_log().lock().unwrap().clear();

    let mut app = UloFactory::create(RpcHookModule).await.unwrap();
    app.use_http_adapter(AxumAdapter::new(), ("127.0.0.1", 0))
        .unwrap();
    // The declared patterns need a transport to reach: `bind()` refuses an RPC controller with
    // no RPC adapter behind it.
    app.use_rpc_adapter(TcpAdapter::new("127.0.0.1", 0))
        .unwrap();
    app.bind().await.unwrap();

    let log = get_log().lock().unwrap().clone();
    assert_eq!(
        log,
        vec!["rpc-controller:init", "rpc-controller:bootstrap"],
        "an RPC controller's startup hooks must fire in the usual order"
    );
}

mod orders_pb {
    tonic::include_proto!("ulo_test.orders");
}

#[ulo_macros::controller]
pub struct HookedGrpcService {}

impl HookedGrpcService {
    #[ulo_macros::new]
    pub fn new() -> Self {
        Self {}
    }

    #[on_module_init]
    async fn ready(&self) -> ulo::InitResult {
        get_log().lock().unwrap().push("grpc-service:init");
        Ok(())
    }

    #[on_application_bootstrap]
    async fn started(&self) -> ulo::InitResult {
        get_log().lock().unwrap().push("grpc-service:bootstrap");
        Ok(())
    }
}

#[ulo_macros::grpc_methods(orders_pb::orders_server::Orders)]
impl HookedGrpcService {
    #[grpc_method]
    async fn create(
        &self,
        Payload(_req): Payload<orders_pb::CreateOrderRequest>,
    ) -> Result<orders_pb::CreateOrderResponse, NotServed> {
        Ok(orders_pb::CreateOrderResponse {
            id: 1,
            status: "ok".to_string(),
        })
    }

    #[grpc_stream]
    async fn watch_progress(
        &self,
        Payload(_req): Payload<orders_pb::WatchRequest>,
    ) -> Result<
        impl futures_util::Stream<Item = Result<orders_pb::ProgressEvent, NotServed>> + Send + 'static,
        NotServed,
    > {
        Ok(futures_util::stream::empty())
    }

    #[grpc_method]
    async fn bulk_create(
        &self,
        _inbound: Inbound<orders_pb::CreateOrderRequest>,
    ) -> Result<orders_pb::BulkCreateResponse, NotServed> {
        Err(NotServed)
    }

    #[grpc_stream]
    async fn chat(
        &self,
        _inbound: Inbound<orders_pb::ChatMessage>,
    ) -> Result<
        impl futures_util::Stream<Item = Result<orders_pb::ChatMessage, NotServed>> + Send + 'static,
        NotServed,
    > {
        Ok(futures_util::stream::empty())
    }
}

#[module(controllers: [HookedGrpcService])]
impl GrpcHookModule {}

/// A gRPC service is kept out of the module's provider map so nothing can inject it, and the
/// startup hooks still reach it. Its hooks are dispatched through the lifecycle bridge rather than
/// by name, because a service built per call has no `Provider` of its own to hang them on — this
/// fails if that rewiring drops them.
#[serial]
#[tokio_localset_test::localset_test]
async fn a_grpc_service_still_gets_its_startup_hooks() {
    get_log().lock().unwrap().clear();

    let mut app = UloFactory::create(GrpcHookModule).await.unwrap();
    app.use_http_adapter(AxumAdapter::new(), ("127.0.0.1", 0))
        .unwrap();
    app.bind().await.unwrap();

    let log = get_log().lock().unwrap().clone();
    assert_eq!(
        log,
        vec!["grpc-service:init", "grpc-service:bootstrap"],
        "a gRPC service's startup hooks must fire in the usual order"
    );
}

#[controller("/ctx")]
pub struct ContextHookedController {}

#[routes]
impl ContextHookedController {
    #[on_module_destroy]
    async fn torn_down(&self) {
        get_log().lock().unwrap().push("controller:destroy");
    }

    #[on_application_shutdown]
    async fn stopped(&self, _signal: Option<String>) {
        get_log().lock().unwrap().push("controller:shutdown");
    }
}

#[module(controllers: [ContextHookedController])]
impl ContextHookModule {}

/// An application context has no HTTP server, and its shutdown hooks used to reach providers only —
/// the controller pass lived on `UloApplication`. Every dispatch target is a controller now, so a
/// worker built with `create_application_context` would otherwise close without running any of them.
#[serial]
#[tokio_localset_test::localset_test]
async fn an_application_context_runs_its_controllers_shutdown_hooks() {
    get_log().lock().unwrap().clear();

    let mut ctx = UloFactory::create_application_context(ContextHookModule)
        .await
        .unwrap();
    ctx.close().await;

    let log = get_log().lock().unwrap().clone();
    assert_eq!(
        log,
        vec!["controller:destroy", "controller:shutdown"],
        "a controller's teardown hooks must run when the context closes"
    );
}

/// The full application delegates its teardown to the context rather than running a controller pass
/// of its own. Each hook must therefore appear once, not twice — a second pass anywhere above the
/// context would show up here as a duplicate.
#[serial]
#[tokio_localset_test::localset_test]
async fn an_application_runs_its_controllers_teardown_hooks_once() {
    get_log().lock().unwrap().clear();

    let mut app = UloFactory::create(ContextHookModule).await.unwrap();
    app.use_http_adapter(AxumAdapter::new(), ("127.0.0.1", 0))
        .unwrap();
    app.bind().await.unwrap();
    app.close().await;

    let log = get_log().lock().unwrap().clone();
    assert_eq!(
        log,
        vec!["controller:destroy", "controller:shutdown"],
        "a controller's teardown hooks must run exactly once when the application closes"
    );
}
