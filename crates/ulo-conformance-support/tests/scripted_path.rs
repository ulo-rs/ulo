//! The suites' path to `ScriptedTimer` and core's name one type.

#[test]
fn the_support_path_is_the_core_type() {
    let clock: ulo::testing::ScriptedTimer = ulo_conformance_support::ScriptedTimer::new();
    assert_eq!(clock.elapsed(), std::time::Duration::ZERO);
}
