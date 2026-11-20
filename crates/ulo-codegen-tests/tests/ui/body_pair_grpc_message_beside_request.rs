// One body consumer per handler (transports DESIGN §2.2): a gRPC `Message` beside `Request`.
// Refused at: the second parameter's type, naming both parameters.
use ulo::{controller, handlers};
use ulo_codegen_tests::pb::{self, AddRequest, Sum};
use ulo_grpc::{Message, Request};

#[controller]
struct Calls;

#[handlers]
impl Calls {
    #[ulo_grpc::method(pb::counter::Add)]
    fn call(&self, req: Message<AddRequest>, again: Request<AddRequest>) -> Sum {
        let _ = (req, again);
        Sum { sum: 0 }
    }
}

fn main() {}
