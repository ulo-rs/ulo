// A `#[controller]` struct whose field carries `#[controller(default)]`. Decision 69.
// Refused at: the field's marker, naming `#[construct(default)]`.
use ulo::controller;

#[controller]
struct Checkout {
    #[controller(default)]
    calls: u32,
}

fn main() {}
