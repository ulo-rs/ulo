// One body consumer per handler (transports DESIGN §2.2): two gRPC `Streaming` parameters on a client-streaming method.
// Refused at: the second parameter's type, naming both parameters.
use ulo::{controller, handlers};
use ulo_codegen_tests::pb::{self, Number, Sum};
use ulo_grpc::{Streaming};

#[controller]
struct Calls;

#[handlers]
impl Calls {
    #[ulo_grpc::method(pb::counter::Total)]
    fn call(&self, numbers: Streaming<Number>, again: Streaming<Number>) -> Sum {
        let _ = (numbers, again);
        Sum { sum: 0 }
    }
}

fn main() {}
