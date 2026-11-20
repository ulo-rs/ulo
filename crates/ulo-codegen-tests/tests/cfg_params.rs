//! A handler parameter behind a `#[cfg]` that is off exists in the source alone, on each of the four
//! transports: its type is declared under the same `cfg`, so any item a transport attribute writes
//! naming the parameter fails to compile, and the app wires without the dependency the parameter
//! would read. A `cfg` reached through `cfg_attr` gates the parameter as a bare one does, and a
//! parameter whose `cfg` is on, `cfg(test)` in this file's build, is extracted and passed, so a
//! gate written as always off leaves the call one argument short. `ulo-http`'s `extract.rs` serves
//! a call through such a handler.

use ulo::{App, Module, ModuleDef, ModuleIdentity, controller, handlers};
use ulo_codegen_tests::pb::{self, AddRequest, Sum};
use ulo_ws::WsModule;

/// Declared under the `cfg` the parameters carry, so no build of this file has it.
#[cfg(any())]
struct Absent;

#[controller]
struct Pages;

#[handlers]
impl Pages {
    #[ulo_http::get("/notes/{id}")]
    fn read(
        &self,
        #[cfg(any())] absent: ulo_http::Path<Absent>,
        id: ulo_http::Path<u32>,
        #[cfg(any())] dep: ulo::Dep<Absent>,
        #[cfg_attr(test, cfg(any()))] through: ulo::Dep<Absent>,
        #[cfg(test)] _exec: ulo::ExecutionRef,
    ) -> String {
        id.0.to_string()
    }
}

#[controller]
struct Bus;

#[handlers]
impl Bus {
    #[ulo_rpc::message("notes.create")]
    fn create(
        &self,
        #[cfg(any())] notes: ulo_rpc::Inbound<Absent>,
        note: ulo_rpc::Payload<String>,
        #[cfg(any())] dep: ulo::Dep<Absent>,
        #[cfg_attr(test, cfg(any()))] through: ulo::Dep<Absent>,
        #[cfg(test)] _exec: ulo::ExecutionRef,
    ) -> String {
        note.0
    }
}

#[controller]
struct Chat;

#[handlers]
#[ulo_ws::gateway(path = "/chat")]
impl Chat {
    #[ulo_ws::message("chat.send")]
    fn send(
        &self,
        #[cfg(any())] absent: ulo_ws::Payload<Absent>,
        text: ulo_ws::Payload<String>,
        #[cfg(any())] dep: ulo::Dep<Absent>,
        #[cfg_attr(test, cfg(any()))] through: ulo::Dep<Absent>,
        #[cfg(test)] _exec: ulo::ExecutionRef,
    ) -> String {
        text.0
    }
}

#[controller]
struct Counter;

#[handlers]
impl Counter {
    #[ulo_grpc::method(pb::counter::Add)]
    fn add(
        &self,
        #[cfg(any())] absent: ulo_grpc::Message<Absent>,
        req: ulo_grpc::Message<AddRequest>,
        #[cfg(any())] dep: ulo::Dep<Absent>,
        #[cfg_attr(test, cfg(any()))] through: ulo::Dep<Absent>,
        #[cfg(test)] _exec: ulo::ExecutionRef,
    ) -> Sum {
        Sum { sum: req.0.a + req.0.b }
    }
}

struct Root;

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.import(WsModule::for_root());
        m.controller::<Pages>();
        m.controller::<Bus>();
        m.controller::<Chat>();
        m.controller::<Counter>();
    }
}

/// A `Dep<Absent>` the wiring read would be a missing dependency: none is reported.
#[tokio::test]
async fn a_handler_whose_parameters_a_cfg_compiles_out_wires_without_them() {
    let app = App::builder(Root).runtime(ulo_runtime_tokio::Tokio::current()).wire().unwrap_or_else(|error| panic!("the app did not wire: {error}"));
    app.connect().await.unwrap_or_else(|error| panic!("the app did not connect: {error}"));
}
