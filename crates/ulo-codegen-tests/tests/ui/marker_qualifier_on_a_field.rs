// A field's `default` marker under a qualifier other than `ulo`, on an `#[injectable]` struct and
// on a `#[controller]` struct. Decision 27 (F595).
// Refused at: the marker's path, with "write the marker bare, or as `ulo::<name>`".
use ulo::{controller, injectable};

#[injectable]
struct Ledger {
    #[wrong::injectable(default)]
    count: u32,
}

#[controller]
struct Till {
    #[framework::controller(default)]
    total: u32,
}

fn main() {}
