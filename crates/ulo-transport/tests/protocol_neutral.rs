//! The shared base names no protocol (transports DESIGN §1): no crate in `ulo-transport`'s
//! dependency tree is an HTTP, HTTP/2, WebSocket or gRPC crate, under any of its features. A
//! transport's typed override of an error's rendering, `ulo_http::WithHttpStatus` or
//! `ulo_ws::WithCloseCode`, lives in that transport's crate.

use std::process::Command;

/// The crates whose types belong to one protocol.
const PROTOCOL_CRATES: &[&str] = &["http", "http-body", "hyper", "h2", "tungstenite", "async-tungstenite", "tonic", "prost"];

/// Every package in `ulo-transport`'s normal dependency tree with all its features on, by name.
///
/// Not `--offline`: `--all-features` resolves optional crates no other build in the job fetched, and
/// `cargo tree` downloads their manifests.
fn dependency_names() -> Vec<String> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    let manifest = concat!(env!("CARGO_MANIFEST_DIR"), "/../../Cargo.toml");
    let output = Command::new(cargo)
        .args(["tree", "--manifest-path", manifest, "-p", "ulo-transport", "-e", "normal", "--all-features"])
        .args(["--prefix", "none", "--format", "{p}", "--locked"])
        .output()
        .expect("`cargo tree` runs");
    assert!(output.status.success(), "`cargo tree` failed: {}", String::from_utf8_lossy(&output.stderr));
    let tree = String::from_utf8(output.stdout).expect("`cargo tree` writes UTF-8");
    tree.lines().filter_map(|line| line.split_whitespace().next()).map(str::to_owned).collect()
}

#[test]
fn no_protocol_crate_is_in_the_base_s_dependency_tree() {
    let names = dependency_names();
    assert!(names.iter().any(|name| name == "ulo-transport"), "the tree lists the crate itself: {names:?}");
    let protocol: Vec<&String> = names.iter().filter(|name| PROTOCOL_CRATES.contains(&name.as_str())).collect();
    assert!(protocol.is_empty(), "`ulo-transport` depends on {protocol:?}, which name one protocol's types");
}
