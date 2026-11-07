// A `#[controller]` struct whose field carries `#[injectable(default)]`. Decision 28.
// Refused at: the field's marker, naming `#[controller(default)]`.
use ulo::controller;

#[controller]
struct Checkout {
    #[injectable(default)]
    calls: u32,
}

fn main() {}
