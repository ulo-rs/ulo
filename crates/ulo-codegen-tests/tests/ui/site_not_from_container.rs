// §12 row: "A field or parameter whose type is not `FromContainer`" (compile). Capabilities 11, 46.
// Refused at: the field's type, by the `FromContainer` bound `#[injectable]` asserts per field.
use ulo::injectable;

#[injectable]
struct Orders {
    table: String,
}

fn main() {}
