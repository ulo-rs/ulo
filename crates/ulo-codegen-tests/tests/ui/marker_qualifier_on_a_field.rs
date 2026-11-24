// A field's `#[construct(default)]` under a qualifier other than `ulo`, on an `#[injectable]`
// struct and on a `#[controller]` struct. Decision 27 (F595).
// Refused at: the marker's path, with "write the marker bare, or as `ulo::construct`".
use ulo::{controller, injectable};

#[injectable]
struct Ledger {
    #[wrong::construct(default)]
    count: u32,
}

#[controller]
struct Till {
    #[framework::construct(default)]
    total: u32,
}

fn main() {}
