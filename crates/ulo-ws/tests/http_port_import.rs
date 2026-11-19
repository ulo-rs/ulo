//! A gateway on the HTTP server's port is served through the hand-off `WsModule` registers, so
//! `wire()` refuses one in an application that imports no `WsModule`, naming the import to add. A
//! gateway declared `port = own` is served by a standalone server and needs no `WsModule`.

use ulo::{App, Module, ModuleDef, ModuleIdentity, StartupError, WiringError, controller, handlers};
use ulo_runtime_tokio::Tokio;
use ulo_ws::WsModule;

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

struct Root(fn(&mut ModuleDef<'_>));

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        (self.0)(m);
    }
}

#[allow(clippy::result_large_err, reason = "the error type is public, so its size is an API decision rather than this function's")]
fn wire(register: fn(&mut ModuleDef<'_>)) -> Result<App<ulo::Wired>, StartupError> {
    App::builder(Root(register)).runtime(Tokio::current()).wire()
}

#[tokio::test]
async fn a_gateway_on_the_http_port_with_no_ws_module_is_refused_at_wire_naming_the_import() {
    let Err(StartupError::Wiring(errors)) = wire(|m| {
        m.controller::<Hall>();
    }) else {
        panic!("a gateway on the HTTP port wired with no `WsModule` imported")
    };
    let entries: Vec<&WiringError> = errors.iter().collect();
    let [WiringError::NotImported { import, consumer, .. }] = entries.as_slice() else {
        panic!("one `NotImported` entry, got: {errors}")
    };
    assert_eq!(*import, "ulo_ws::WsModule::for_root()");
    assert!(consumer.contains("Hall"), "the entry names the gateway: {consumer}");
    let report = errors.to_string();
    assert!(report.contains("help: add `ulo_ws::WsModule::for_root()` to the `imports` of a module of the application"), "{report}");
}

#[tokio::test]
async fn a_gateway_on_the_http_port_wires_with_ws_module_imported() {
    wire(|m| {
        m.import(WsModule::for_root());
        m.controller::<Hall>();
    })
    .unwrap_or_else(|error| panic!("{error}"));
}

#[tokio::test]
async fn a_gateway_on_its_own_port_wires_with_no_ws_module() {
    wire(|m| {
        m.controller::<Lobby>();
    })
    .unwrap_or_else(|error| panic!("{error}"));
}
