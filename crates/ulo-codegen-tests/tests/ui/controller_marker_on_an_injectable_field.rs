// An `#[injectable]` struct whose field carries `#[controller(default)]`. Decision 28.
// Refused at: the field's marker, naming `#[injectable(default)]`.
use ulo::injectable;

#[injectable]
struct Orders {
    #[controller(default)]
    limit: u32,
}

fn main() {}
