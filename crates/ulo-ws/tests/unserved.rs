//! A gateway is a `Ws` controller served by the server its `port` names, and `listen()` refuses
//! one no bound server serves: a gateway declared `port = own` beside the HTTP server alone, with
//! no standalone WebSocket server, is one `Configure` entry naming the gateway and `Ws`.

use ulo::{App, Module, ModuleDef, ModuleIdentity, PrepareError, Signal, StartupError, TypeName, controller, handlers};
use ulo_runtime_tokio::Tokio;
use ulo_ws::{Ws, WsModule};

#[controller]
struct Lobby;

#[handlers]
#[ulo_ws::gateway(path = "/lobby", port = own)]
impl Lobby {
    #[ulo_ws::message("echo")]
    fn echo(&self, text: ulo_ws::Payload<String>) -> String {
        text.0
    }
}

#[controller]
struct Hall;

#[handlers]
#[ulo_ws::gateway(path = "/hall")]
impl Hall {
    #[ulo_ws::message("echo")]
    fn echo(&self, text: ulo_ws::Payload<String>) -> String {
        text.0
    }
}

struct Root(fn(&mut ModuleDef<'_>));

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.import(WsModule::for_root());
        (self.0)(m);
    }
}

fn own_port(m: &mut ModuleDef<'_>) {
    m.controller::<Lobby>();
}

fn http_port(m: &mut ModuleDef<'_>) {
    m.controller::<Hall>();
}

async fn listen(builder: ulo::AppBuilder) -> Result<(), StartupError> {
    let app = builder.runtime(Tokio::current()).wire()?.connect().await?;
    let bound = app.bind(ulo_http_hyper::Server::new("127.0.0.1:0")).listen().await?;
    let _ = bound.handle().close(Signal::new("test")).await;
    Ok(())
}

#[tokio::test]
async fn a_gateway_declared_port_own_beside_the_http_server_alone_is_refused_naming_it_and_ws() {
    let Err(StartupError::Configure(errors)) = listen(App::builder(Root(own_port))).await else {
        panic!("a `port = own` gateway with no standalone server listened")
    };

    assert_eq!(errors.len(), 1, "{errors}");
    let entry = errors.iter().next().unwrap();
    assert_eq!(entry.transport, TypeName::of::<Ws>(), "{errors}");
    let error = entry.source.downcast_ref::<PrepareError>().expect("a `PrepareError`");
    assert_eq!(error.names(), [TypeName::of::<Lobby>(), TypeName::of::<Ws>(), TypeName::of::<Ws>()], "{errors}");
    assert!(entry.to_string().contains("the controller `Lobby`"), "{errors}");
    assert!(entry.to_string().contains("`AppBuilder::unserved::<Ws>()`"), "{errors}");
}

#[tokio::test]
async fn a_port_own_gateway_passes_where_the_binary_serves_no_ws() {
    listen(App::builder(Root(own_port)).unserved::<Ws>()).await.unwrap_or_else(|error| panic!("{error}"));
}

#[tokio::test]
async fn a_gateway_on_the_http_port_is_served_by_the_http_server() {
    listen(App::builder(Root(http_port))).await.unwrap_or_else(|error| panic!("{error}"));
}
