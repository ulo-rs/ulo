// §12 row: "Two or zero constructors, a bad attribute, a malformed `into` item" (compile), a bad
// field attribute. Capability 46.
// Refused at: the field's marker, which takes `#[injectable(default)]` alone.
use ulo::injectable;

#[injectable]
struct Orders {
    #[injectable(singleton)]
    limit: u32,
}

fn main() {}
