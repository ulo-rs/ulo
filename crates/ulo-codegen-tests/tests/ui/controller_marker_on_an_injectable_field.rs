// An `#[injectable]` struct whose field carries `#[controller(default)]`. Decision 69.
// Refused at: the field's marker, naming `#[construct(default)]`.
use ulo::injectable;

#[injectable]
struct Orders {
    #[controller(default)]
    limit: u32,
}

fn main() {}
