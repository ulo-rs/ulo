// §12 row: "A constructor that isn't `Send`" (compile). Capability 46.
// Refused at: the constructor fn, where the generated `Construct` impl needs a `Send` future.
use std::rc::Rc;

use ulo::injectable;

struct Orders;

#[injectable]
impl Orders {
    async fn new() -> Self {
        let held = Rc::new(1);
        std::future::ready(()).await;
        let _ = held;
        Orders
    }
}

fn main() {}
