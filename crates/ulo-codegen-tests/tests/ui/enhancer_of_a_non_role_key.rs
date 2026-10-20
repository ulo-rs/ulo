// §12 row: "`enhancer::<K>()` with a `K` that is not a role key" (compile). Capabilities 28, 46.
// Refused at: the `enhancer::<K>` call, by the `Role` bound.
use ulo::{Module, ModuleDef, ModuleIdentity};

trait Plugin: Send + Sync {}

struct Plugins;

impl Module for Plugins {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.enhancer::<dyn Plugin>();
    }
}

fn main() {}
