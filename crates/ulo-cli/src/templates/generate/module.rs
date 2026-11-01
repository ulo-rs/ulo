use ulo::{Module, ModuleDef, ModuleIdentity};

/// Groups what belongs together: bind its providers and controllers in `register`, and export what
/// other modules may use. Another module imports it with `m.import(@@pascal@@Module)`.
pub struct @@pascal@@Module;

impl Module for @@pascal@@Module {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, _m: &mut ModuleDef<'_>) {}
}

#[cfg(test)]
mod tests {
    use ulo::TestApp;

    use super::@@pascal@@Module;

    #[test]
    fn wires() {
        TestApp::of(@@pascal@@Module).wire().expect("the module wires");
    }
}
