// X24's refusal holds with a user trait in scope whose `check` every type has: the probe's method
// is not the user's, so a metadata type naming HTTP on an RPC handler still fails at the value.
use std::time::Duration;

use ulo::{injectable, routes};
use ulo_http::Timeout;
use ulo_rpc::Payload;

#[allow(dead_code)]
trait Check {
    fn check(&self) {}
}

impl<T: ?Sized> Check for T {}

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

fn main() {}
