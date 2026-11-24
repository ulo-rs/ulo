// A `#[controller]` struct whose field carries `#[injectable(default)]`. Decision 69.
// Refused at: the field's marker, naming `#[construct(default)]`.
use ulo::controller;

#[controller]
struct Checkout {
    #[injectable(default)]
    calls: u32,
}

fn main() {}
