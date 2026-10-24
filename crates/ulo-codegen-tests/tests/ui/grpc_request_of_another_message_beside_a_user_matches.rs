// The request check holds with a user trait in scope whose `matches` every type has: the check's
// method is not the user's, so a handler taking another message than the method's still fails.
use ulo::{injectable, routes};
use ulo_codegen_tests::pb::{self, Number, Sum};
use ulo_grpc::Message;

#[allow(dead_code)]
trait Matches {
    fn matches(&self) {}
}

impl<T: ?Sized> Matches for T {}

#[injectable]
struct Counter;

#[routes]
impl Counter {
    // `Add` takes an `AddRequest`.
    #[ulo_grpc::method(pb::counter::Add)]
    async fn add(&self, req: Message<Number>) -> Sum {
        Sum { sum: req.0.value }
    }
}

fn main() {}
