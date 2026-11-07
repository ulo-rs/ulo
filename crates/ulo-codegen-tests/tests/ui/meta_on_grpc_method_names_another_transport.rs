use ulo::{controller, handlers};
use ulo_codegen_tests::pb::{self, AddRequest, Sum};
use ulo_grpc::Message;
use ulo_http::BodyLimit;

#[controller]
struct Counter;

#[handlers]
impl Counter {
    #[ulo_grpc::method(pb::counter::Add)]
    #[meta(BodyLimit(1024))]
    async fn add(&self, req: Message<AddRequest>) -> Sum {
        Sum { sum: req.0.a + req.0.b }
    }
}

fn main() {}
