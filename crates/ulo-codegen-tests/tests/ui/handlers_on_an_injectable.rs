// A `#[handlers]` impl on an `#[injectable]` type, whose handlers no module would mount.
// Decision 28.
// Refused at: the `#[handlers]` impl, naming the type as not a controller and pointing at
// `#[controller]`.
use ulo::{handlers, injectable};

#[injectable]
struct Checkout;

#[handlers]
impl Checkout {
    #[ulo_http::get("/checkout")]
    fn get(&self) -> &'static str {
        "checkout"
    }
}

fn main() {}
