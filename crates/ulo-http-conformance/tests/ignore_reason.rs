//! A scenario declared not applicable is reported ignored with its reason: libtest prints an
//! `ignore` reason only when the attribute carries a string literal, so the stamper must not
//! build it with a macro. The test runs this binary again on the one declared scenario and reads
//! the line libtest writes for it.

use std::process::Command;

use ulo_http_conformance::{HyperHost, OnTokio};

const REASON: &str = "the reference has no host around the app to read `Routing`";

ulo_http_conformance::http_conformance_suite!(HyperHost<OnTokio>; not_applicable {
    routing_extension: "the reference has no host around the app to read `Routing`",
});

#[test]
fn a_declared_scenario_is_reported_ignored_with_its_reason() {
    let output = Command::new(std::env::current_exe().expect("the test binary's path"))
        .args(["--exact", "nested::routing_extension", "--test-threads=1"])
        .output()
        .expect("the test binary runs again");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = format!("test nested::routing_extension ... ignored, {REASON}");
    assert!(stdout.lines().any(|found| found == line), "libtest's report lacks `{line}`:\n{stdout}");
}
