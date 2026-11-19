use ulo::module;

/// Groups what belongs together. List its parts in the attribute,
/// `#[module(imports = [..], providers = [..], controllers = [..], exports = [..])]`; another
/// module imports it by naming `@@pascal@@Module` in its own `imports`, or `m.import(@@pascal@@Module)`.
#[module]
pub struct @@pascal@@Module;

#[cfg(test)]
mod tests {
    use ulo::TestApp;

    use super::@@pascal@@Module;

    #[test]
    fn wires() {
        TestApp::of(@@pascal@@Module).wire().expect("the module wires");
    }
}
