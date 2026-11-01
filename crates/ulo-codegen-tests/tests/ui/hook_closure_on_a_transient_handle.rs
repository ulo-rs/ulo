// §12 row: "A closure hook or `.ready(..)` on an execution-scoped or transient handle" (compile),
// the closure-hook trigger on a transient handle. Capability 46.
// Refused at: the `.on_init` call, by `HookHost` on the handle's binding state.
use ulo::{BoxError, Module, ModuleDef, ModuleIdentity};

struct Scratch;

struct Pads;

impl Module for Pads {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.transient(|| async { Scratch }).on_init(|| async { Ok::<(), BoxError>(()) });
    }
}

fn main() {}
