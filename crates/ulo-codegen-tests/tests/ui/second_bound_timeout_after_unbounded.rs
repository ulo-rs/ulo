// §12 row: "A second bound on one binding-handle item" (compile), `.timeout` after
// `.unbounded()`. Capability 46.
// Refused at: the `.timeout` call, which the item's bounded state does not have (E0599).
use std::time::Duration;

use ulo::{Module, ModuleDef, ModuleIdentity};

struct Pool;

struct Db;

impl Module for Db {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.singleton(|| async { Pool }).unbounded().timeout(Duration::from_secs(1));
    }
}

fn main() {}
