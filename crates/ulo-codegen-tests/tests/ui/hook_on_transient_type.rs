// §12 row: "A hook on an explicit execution-scoped or transient type" (compile), the transient
// trigger. Capabilities 41, 46.
// Refused at: the `OnModuleDestroy` impl, by `Construct<Scope: HookCapable>`.
use ulo::{OnModuleDestroy, injectable};

#[injectable(transient)]
struct Scratch;

impl OnModuleDestroy for Scratch {
    async fn on_module_destroy(&self) {}
}

fn main() {}
