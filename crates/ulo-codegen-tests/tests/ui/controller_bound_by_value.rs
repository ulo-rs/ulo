// A `#[controller]` value listed under `providers`: braced, built by a call, fallible, and as an
// `into` item's `value`, plain and fallible. Decision 28.
// Refused at: each entry's expression, naming the type as a controller that goes under `controllers`.
use ulo::{controller, handlers, module};

trait Plugin: Send + Sync {}

#[controller]
struct Checkout;

impl Plugin for Checkout {}

impl Checkout {
    fn open() -> Result<Self, std::io::Error> {
        Ok(Checkout)
    }
}

#[handlers]
impl Checkout {
    #[ulo_http::get("/checkout")]
    fn get(&self) -> &'static str {
        "checkout"
    }
}

#[module(providers = [{ Checkout }, Checkout::open()?, into dyn Plugin: [value = Checkout, value = Checkout::open()?]])]
struct Shop;

fn main() {}
