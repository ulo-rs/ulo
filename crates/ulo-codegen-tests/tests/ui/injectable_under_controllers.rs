// An `#[injectable]` type listed under `controllers`. Decision 28.
// Refused at: the entry, naming the type as not a controller and pointing at `#[controller]`.
use ulo::{injectable, module};

#[injectable]
struct Checkout;

#[module(controllers = [Checkout])]
struct Shop;

fn main() {}
