//! Programs the transport attributes must refuse, each beside the diagnostic it gets:
//! metadata of one transport on another transport's handler (X24), on RPC, WebSocket and gRPC
//! handlers, and gRPC handlers whose signatures disagree with their method marker, in shape or in
//! message type. One further case builds a crate with its features off: `ulo-otel-otlp` without an
//! HTTP client must answer one error, the `compile_error!` naming the two features.
//!
//! The `.stderr` snapshots hold rustc's own wording, which changes between releases, so the test
//! runs only on the compiler that wrote them; `build.rs` sets `ulo_snapshot_rustc` there. On any
//! other compiler it is reported ignored. Regenerate on that compiler after an intended change:
//!
//! ```text
//! TRYBUILD=overwrite cargo +1.98.1 test -p ulo-codegen-tests --test compile_fail
//! ```

#[test]
#[cfg_attr(not(ulo_snapshot_rustc), ignore = "the .stderr snapshots are rustc 1.98.1's")]
fn mismatched_handlers_fail_to_compile() {
    trybuild::TestCases::new().compile_fail("tests/ui/*.rs");
}

/// `ulo-otel-otlp` built with neither client feature answers the `compile_error!` and no other
/// error, none from an item that names a client. A warning cannot show here: rustc runs no late
/// lint once it has reported an error. The crate is a dependency of no test crate here, since a build of it without a client
/// fails by design, so the case runs `cargo check` on it and compares what cargo prints to
/// `tests/ui_features/otlp_without_a_client.stderr`, with the progress lines taken out.
#[test]
#[cfg_attr(not(ulo_snapshot_rustc), ignore = "the .stderr snapshots are rustc 1.98.1's")]
fn a_client_less_otlp_exporter_fails_with_one_error() {
    use std::path::Path;
    use std::process::Command;

    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let output = Command::new(std::env::var_os("CARGO").expect("cargo sets CARGO for a test"))
        .current_dir(&workspace)
        .args(["check", "-p", "ulo-otel-otlp", "--no-default-features", "--color", "never"])
        .output()
        .expect("cargo runs");
    assert!(!output.status.success(), "a build with no HTTP client succeeded");

    let stderr = String::from_utf8(output.stderr).expect("cargo writes UTF-8");
    let progress = ["Compiling ", "Checking ", "Finished ", "Blocking ", "Updating ", "Locking ", "Downloaded ", "Downloading "];
    let printed: String =
        stderr.lines().filter(|line| !progress.iter().any(|verb| line.trim_start().starts_with(verb))).map(|line| format!("{line}\n")).collect();

    let snapshot = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/ui_features/otlp_without_a_client.stderr");
    if std::env::var_os("TRYBUILD").is_some_and(|mode| mode == "overwrite") {
        std::fs::write(&snapshot, &printed).expect("write the snapshot");
    }
    let expected = std::fs::read_to_string(&snapshot).expect("read the snapshot");
    assert_eq!(printed, expected, "cargo's output for a client-less build changed");
}
