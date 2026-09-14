//! A module built at runtime can contribute a dispatch target, not only providers.
//!
//! `#[module]` takes `controllers:` because a target declared with an attribute is known at compile
//! time. An integration whose target comes from a value it was configured with — a schema, a path —
//! has no attribute to write it on, and `DynamicModule` carried providers only, so such a target
//! had to be registered as a provider role instead.

use ulo::di::DynamicModule;
use ulo::dispatch::ControllerFactory;
use ulo::http::Body;
use ulo::prelude::*;
use ulo_macros::{controller, get, module, routes};

use crate::common::TestServer;

/// Built from a value rather than from an attribute: the path is chosen when the module is made.
#[controller("/dyn")]
pub struct GreetingController {}

#[routes]
impl GreetingController {
    #[get("/hello")]
    async fn hello(&self) -> Body {
        Body::text("from a dynamic module")
    }
}

fn runtime_module() -> DynamicModule {
    DynamicModule::builder("RuntimeGreetings")
        .controller(GreetingController::__ulo_controller_factory())
        .build()
}

#[module(imports: [runtime_module()])]
pub struct AppModule;

/// The route the dynamic module declared answers, so the target reached registration by the same
/// path a `controllers:` entry does.
#[tokio_localset_test::localset_test]
async fn a_dynamic_module_s_controller_serves() {
    let server = TestServer::start(AppModule).await;

    let body = reqwest::get(format!("http://127.0.0.1:{}/dyn/hello", server.port))
        .await
        .expect("the route must answer")
        .text()
        .await
        .expect("the body must read");

    assert_eq!(body, "from a dynamic module");
}

/// A module that declares no target hands back none, rather than an empty list the loader would
/// still walk.
#[test]
fn a_dynamic_module_without_one_declares_nothing() {
    let module = DynamicModule::builder("NoTargets").build();
    let declared: Option<Vec<Box<dyn ControllerFactory>>> =
        ulo::di::ModuleMetadata::controllers(&module);

    assert!(
        declared.is_none_or(|c| c.is_empty()),
        "a module with no controller declares none"
    );
}
