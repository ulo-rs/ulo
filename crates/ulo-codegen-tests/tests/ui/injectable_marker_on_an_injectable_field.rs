// An `#[injectable]` struct whose field carries `#[injectable(default)]`, bare and under `ulo`,
// and `#[injectable(..)]` with another argument. Decision 69.
// Refused at: each field's marker, naming `#[construct(default)]`.
use ulo::injectable;

#[injectable]
struct Orders {
    #[injectable(default)]
    limit: u32,
}

#[injectable]
struct Ledger {
    #[ulo::injectable(default)]
    count: u32,
}

#[injectable]
struct Clerk {
    #[injectable(singleton)]
    shift: u32,
}

fn main() {}
