// §12 row: "A closure hook or `.ready(..)` on an execution-scoped or transient handle" (compile),
// the `.ready(..)` trigger on an execution handle. Capability 46.
// Refused at: the `.ready` call, by `HookHost` on the handle's binding state.
use ulo::{BoxError, Module, ModuleDef, ModuleIdentity};

struct Gauge;

struct Metrics;

impl Module for Metrics {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.execution(|| async { Gauge }).ready(|| async { Ok::<(), BoxError>(()) });
    }
}

fn main() {}
