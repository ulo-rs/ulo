// §12 row: "A second bound on one binding-handle item" (compile), `.unbounded()` beside
// `.attempt_timeout` on a readiness check. Capability 46.
// Refused at: the `.unbounded()` call, which a readiness check with one bound written does not
// have (E0599).
use std::time::Duration;

use ulo::{BoxError, Module, ModuleDef, ModuleIdentity};

struct Pool;

struct Db;

impl Module for Db {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.singleton(|| async { Pool })
            .ready(|| async { Ok::<(), BoxError>(()) })
            .attempt_timeout(Duration::from_secs(1))
            .unbounded();
    }
}

fn main() {}
