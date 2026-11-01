// §12 row: "`Ext` / `ExecutionRef` in an explicit singleton" (compile), the `Ext` trigger.
// Capability 46.
// Refused at: the field's type, by `AllowedIn<Singleton>`.
use ulo::{Ext, injectable};

struct CurrentUser;

#[injectable(singleton)]
struct Audit {
    user: Ext<CurrentUser>,
}

fn main() {}
