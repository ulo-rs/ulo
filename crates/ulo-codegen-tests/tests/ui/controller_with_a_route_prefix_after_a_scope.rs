// `#[controller]` given a route prefix after its scope. Decision 28.
// Refused at: the prefix, pointing at the module's `.at(prefix)`.
use ulo::controller;

#[controller(singleton, "/checkout")]
struct Checkout;

fn main() {}
