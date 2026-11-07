use ulo::{controller, handlers};
use ulo_codegen_tests::pb::{self, CountRequest, Number};
use ulo_grpc::Message;

#[controller]
struct Counter;

#[handlers]
impl Counter {
    // `Count` streams its reply.
    #[ulo_grpc::method(pb::counter::Count)]
    async fn count(&self, req: Message<CountRequest>) -> Number {
        Number { value: i64::from(req.0.up_to) }
    }
}

fn main() {}
