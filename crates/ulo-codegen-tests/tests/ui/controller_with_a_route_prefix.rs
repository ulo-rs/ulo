// `#[controller]` given a route prefix. Decision 28.
// Refused at: the prefix, pointing at the module's `.at(prefix)`.
use ulo::controller;

#[controller("/checkout")]
struct Checkout;

fn main() {}
