// `#[construct]` on a field with no argument, and with `default` given a value. Decision 69.
// Refused at: each marker, "a field takes `#[construct(default)]` alone".
use ulo::{controller, injectable};

#[injectable]
struct Orders {
    #[construct]
    limit: u32,
}

#[controller]
struct Checkout {
    #[construct(default = 3)]
    calls: u32,
}

fn main() {}
