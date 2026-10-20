// §12 row: "Two or zero constructors, a bad attribute, a malformed `into` item" (compile), a bad
// attribute argument. Capability 46.
// Refused at: the argument, which is neither a scope nor `timeout = ..`.
use ulo::injectable;

#[injectable(forever)]
struct Orders;

fn main() {}
