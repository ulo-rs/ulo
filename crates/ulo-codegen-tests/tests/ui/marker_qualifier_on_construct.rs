// The `#[construct]` mark `#[injectable]` and `#[controller]` read, under a qualifier other than
// `ulo`. Decision 27 (F595).
// Refused at: the mark's path, with "write the marker bare, or as `ulo::construct`".
use ulo::{controller, injectable};

struct Ledger;

#[injectable]
impl Ledger {
    #[wrong::construct]
    fn build() -> Self {
        Ledger
    }
}

struct Till;

#[controller]
impl Till {
    #[ulo::x::construct]
    fn build() -> Self {
        Till
    }
}

fn main() {}
