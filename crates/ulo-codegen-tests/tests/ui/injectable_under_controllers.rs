// An `#[injectable]` type with a `#[handlers]` impl, listed under `controllers`. Decision 28.
// Refused at: the entry, naming the type and pointing at `#[controller]`.
use ulo::{handlers, injectable, module};

#[injectable]
struct Checkout;

#[handlers]
impl Checkout {
    #[ulo_http::get("/checkout")]
    fn get(&self) -> &'static str {
        "checkout"
    }
}

#[module(controllers = [Checkout])]
struct Shop;

fn main() {}
