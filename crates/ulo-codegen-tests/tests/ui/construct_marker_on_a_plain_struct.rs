// `#[construct(default)]` on a field of a struct carrying neither `#[injectable]` nor
// `#[controller]`, with `construct` in scope and without. Decision 69.
// Refused by rustc: no attribute reads the marker, so it is an attribute macro on a field where it
// is imported, and an unknown attribute where it is not.
mod imported {
    use ulo::construct;

    pub struct Orders {
        #[construct(default)]
        pub limit: u32,
    }
}

pub struct Ledger {
    #[construct(default)]
    pub count: u32,
}

fn main() {}
