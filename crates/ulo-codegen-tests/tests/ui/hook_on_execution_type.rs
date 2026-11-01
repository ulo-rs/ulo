// §12 row: "A hook on an explicit execution-scoped or transient type" (compile), the execution
// trigger. Capabilities 41, 46.
// Refused at: the `OnModuleInit` impl, by `Construct<Scope: HookCapable>`.
use ulo::{BoxError, OnModuleInit, injectable};

#[injectable(execution)]
struct Session;

impl OnModuleInit for Session {
    async fn on_module_init(&self) -> Result<(), BoxError> {
        Ok(())
    }
}

fn main() {}
