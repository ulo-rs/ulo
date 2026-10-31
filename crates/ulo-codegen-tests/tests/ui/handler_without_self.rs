// Inventory G13 (`static_methods`): a handler is a method. One written without a receiver is
// refused at its name: "a handler is a method: it takes `&self`, or `self: Arc<Self>`". Example 15's
// README points here. The two E0599 errors after it name items the attribute did not write for the
// refused handler.
use ulo::{injectable, routes};

#[injectable]
struct Status;

#[routes]
impl Status {
    #[ulo_http::get("/status")]
    fn status() -> &'static str {
        "ok"
    }
}

fn main() {}
