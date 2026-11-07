// A `#[controller]` type listed under `providers`, as a type, behind `as dyn K`, qualified, and as
// an `into` item. Decision 28.
// Refused at: each entry's type, naming it as a controller that goes under `controllers`.
use ulo::{controller, handlers, module};

trait Plugin: Send + Sync {}

#[controller]
struct Checkout;

impl Plugin for Checkout {}

#[handlers]
impl Checkout {
    #[ulo_http::get("/checkout")]
    fn get(&self) -> &'static str {
        "checkout"
    }
}

struct Primary;

#[module(providers = [Checkout, Checkout as dyn Plugin, Checkout @ Primary, into dyn Plugin: [Checkout]])]
struct Shop;

fn main() {}
