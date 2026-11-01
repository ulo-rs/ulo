// §12 row: "`Dep<T>` with `T` not `Send + Sync`" (compile). Capability 46.
// Refused at: the field's type, by the bound on `Dep`'s `FromContainer` impl.
use std::rc::Rc;

use ulo::{Dep, injectable};

#[injectable]
struct Orders {
    cache: Dep<Rc<u8>>,
}

fn main() {}
