use ulo::{controller, handlers};
use ulo_rpc::Payload;

#[controller]
struct Orders;

#[handlers]
impl Orders {
    #[ulo_rpc::message("orders.get", every_instance)]
    async fn get(&self, id: Payload<u64>) -> u64 {
        id.0
    }
}

fn main() {}
