// Every refusal a generator exists to make holds with a user trait in scope whose methods `check`,
// `matches` and `construct` every type has: the probes the generators emit are `__ulo_` names
// called by path, so none of them resolves to the user's method and none of the checks turns off.
// Four refusals, one per generator family: metadata naming another transport (`check`), a gRPC
// handler taking another message than its method (`matches`), a gRPC handler replying another
// message, and a hook on a type that is built per use (`construct`).
use std::time::Duration;

use ulo::{OnModuleDestroy, injectable, routes};
use ulo_codegen_tests::pb::{self, AddRequest, Number, Sum};
use ulo_grpc::Message;
use ulo_http::Timeout;
use ulo_rpc::Payload;

#[allow(dead_code)]
trait Blanket {
    fn check(&self) {}
    fn matches(&self) {}
    fn construct(&self) {}
}

impl<T: ?Sized> Blanket for T {}

#[injectable]
struct Orders;

#[routes]
impl Orders {
    #[ulo_rpc::message("orders.get")]
    #[meta(Timeout::after(Duration::from_secs(5)))]
    async fn get(&self, id: Payload<u64>) -> u64 {
        id.0
    }
}

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

#[injectable]
struct Replier;

#[routes]
impl Replier {
    // `Add` replies a `Sum`.
    #[ulo_grpc::method(pb::counter::Add)]
    async fn add(&self, req: Message<AddRequest>) -> Number {
        Number { value: req.0.a + req.0.b }
    }
}

#[injectable(transient)]
struct Scratch;

impl OnModuleDestroy for Scratch {
    async fn on_module_destroy(&self) {}
}

fn main() {}
