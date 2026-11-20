//! A gateway declared `port = own` is served by the standalone server, and an HTTP route the
//! same controller carries by the HTTP server: `listen()` with the standalone server alone refuses
//! the route, naming the controller and `Http`, and passes the gateway.

use ulo::{App, Module, ModuleDef, ModuleIdentity, PrepareError, Signal, StartupError, TypeName, controller, handlers};
use ulo_runtime_tokio::Tokio;
use ulo_ws::WsModule;

#[controller]
struct Kiosk;

#[handlers]
#[ulo_ws::gateway(path = "/kiosk", port = own)]
impl Kiosk {
    #[ulo_ws::message("echo")]
    fn echo(&self, text: ulo_ws::Payload<String>) -> String {
        text.0
    }

    #[ulo_http::get("/kiosk/status")]
    fn status(&self) -> &'static str {
        "up"
    }
}

struct Root;

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.import(WsModule::for_root());
        m.controller::<Kiosk>();
    }
}

#[allow(clippy::result_large_err, reason = "the error type is public, so its size is an API decision rather than this function's")]
async fn listen(builder: ulo::AppBuilder) -> Result<(), StartupError> {
    let app = builder.runtime(Tokio::current()).wire()?.connect().await?;
    let bound = app.bind(ulo_ws_hyper::Server::new("127.0.0.1:0")).listen().await?;
    let _ = bound.handle().close(Signal::new("test")).await;
    Ok(())
}

#[tokio::test]
async fn an_http_route_on_a_port_own_gateway_is_refused_beside_the_standalone_server_alone() {
    let Err(StartupError::Configure(errors)) = listen(App::builder(Root)).await else {
        panic!("an HTTP route with no HTTP server listened")
    };

    assert_eq!(errors.len(), 1, "the gateway is served, its HTTP route is not: {errors}");
    let entry = errors.iter().next().unwrap();
    let http = TypeName::of::<ulo_http::Http>();
    assert_eq!(entry.transport, http, "{errors}");
    let error = entry.source.downcast_ref::<PrepareError>().expect("a `PrepareError`");
    assert_eq!(error.names(), [TypeName::of::<Kiosk>(), http, http], "{errors}");
}

#[tokio::test]
async fn the_same_gateway_passes_where_the_binary_serves_no_http() {
    listen(App::builder(Root).unserved::<ulo_http::Http>()).await.unwrap_or_else(|error| panic!("{error}"));
}
