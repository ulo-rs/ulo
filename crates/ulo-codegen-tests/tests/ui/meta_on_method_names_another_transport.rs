use std::time::Duration;

use ulo::{controller, handlers};
use ulo_http::Timeout;
use ulo_rpc::Payload;

#[controller]
struct Orders;

#[handlers]
impl Orders {
    #[ulo_rpc::message("orders.get")]
    #[meta(Timeout::after(Duration::from_secs(5)))]
    async fn get(&self, id: Payload<u64>) -> u64 {
        id.0
    }
}

fn main() {}
