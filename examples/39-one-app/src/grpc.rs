use ulo::{Dep, injectable, routes};
use ulo_grpc::Message;

use crate::store::{Gate, Store};

pub mod pb {
    ulo_grpc::include_proto!("notes.v1");
}

#[injectable]
pub struct GrpcApi {
    store: Dep<Store>,
    gate: Dep<Gate>,
}

#[routes]
impl GrpcApi {
    #[ulo_grpc::method(pb::notes::Add)]
    async fn add(&self, note: Message<pb::Note>) -> pb::Count {
        pb::Count { n: self.store.add("grpc", &note.0.text) }
    }

    #[ulo_grpc::method(pb::notes::Hold)]
    async fn hold(&self, note: Message<pb::Note>) -> pb::Note {
        self.gate.hold().await;
        note.0
    }
}
