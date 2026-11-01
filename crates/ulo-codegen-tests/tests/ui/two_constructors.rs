// §12 row: "Two or zero constructors, a bad attribute, a malformed `into` item" (compile), two
// constructors. Capability 46.
// Refused at: the second `#[construct]`, "a type has one constructor, so mark one fn only".
use ulo::injectable;

struct Orders;

#[injectable]
impl Orders {
    #[construct]
    fn open() -> Self {
        Orders
    }

    #[construct]
    fn reopen() -> Self {
        Orders
    }
}

fn main() {}
