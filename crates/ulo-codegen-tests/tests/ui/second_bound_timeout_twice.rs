// §12 row: "A second bound on one binding-handle item" (compile), `.timeout` twice. Capability 46.
// Refused at: the second `.timeout`, which the item's bounded state does not have (E0599).
use std::time::Duration;

use ulo::{Module, ModuleDef, ModuleIdentity};

struct Pool;

struct Db;

impl Module for Db {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.singleton(|| async { Pool }).timeout(Duration::from_secs(1)).timeout(Duration::from_secs(2));
    }
}

fn main() {}
