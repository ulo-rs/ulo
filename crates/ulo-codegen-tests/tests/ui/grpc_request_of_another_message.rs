use ulo::{controller, handlers};
use ulo_codegen_tests::pb::{self, Number, Sum};
use ulo_grpc::Message;

#[controller]
struct Counter;

#[handlers]
impl Counter {
    // `Add` is unary, as this handler is, and takes an `AddRequest`.
    #[ulo_grpc::method(pb::counter::Add)]
    async fn add(&self, req: Message<Number>) -> Sum {
        Sum { sum: req.0.value }
    }
}

fn main() {}
