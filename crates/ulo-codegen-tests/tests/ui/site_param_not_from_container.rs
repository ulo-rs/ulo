// §12 row: "A field or parameter whose type is not `FromContainer`" (compile), the constructor
// parameter trigger. Capabilities 11, 46.
// Refused at: the parameter's type.
use ulo::injectable;

struct Orders;

#[injectable]
impl Orders {
    fn new(table: String) -> Self {
        let _ = table;
        Orders
    }
}

fn main() {}
