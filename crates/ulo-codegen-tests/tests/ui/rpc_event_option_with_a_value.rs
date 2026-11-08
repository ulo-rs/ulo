use ulo::{controller, handlers};
use ulo_rpc::Payload;

#[controller]
struct Orders;

#[handlers]
impl Orders {
    #[ulo_rpc::event("orders.placed", every_instance = true)]
    async fn placed(&self, id: Payload<u64>) {
        let _ = id;
    }
}

fn main() {}
