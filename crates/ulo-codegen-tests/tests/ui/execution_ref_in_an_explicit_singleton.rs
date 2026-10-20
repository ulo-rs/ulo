// §12 row: "`Ext` / `ExecutionRef` in an explicit singleton" (compile), the `ExecutionRef`
// trigger, read through a constructor parameter. Capability 46.
// Refused at: the parameter's type, by `AllowedIn<Singleton>`.
use ulo::{ExecutionRef, injectable};

struct Audit;

#[injectable(singleton)]
impl Audit {
    fn new(execution: ExecutionRef) -> Self {
        let _ = execution;
        Audit
    }
}

fn main() {}
