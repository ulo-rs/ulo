// §12 row: "Two or zero constructors, a bad attribute, a malformed `into` item" (compile), zero
// constructors. Capability 46.
// Refused at: the impl's self type, "no constructor: name it `new`, or mark the fn that builds
// the type #[construct]".
use ulo::injectable;

struct Orders;

#[injectable]
impl Orders {
    fn open() -> Self {
        Orders
    }
}

fn main() {}
