// §12 row: "`.qualified::<Q>()` on an enhancer contribution" (compile). Capabilities 28, 46.
// Refused at: the `.qualified` call, which `Contribute<.., Enhancer>` does not have.
use ulo::{AnyGuard, Module, ModuleDef, ModuleIdentity};
use ulo_http::Http;

struct Admin;

struct Auth;

impl Module for Auth {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.enhancer::<AnyGuard<Http>>().qualified::<Admin>();
    }
}

fn main() {}
