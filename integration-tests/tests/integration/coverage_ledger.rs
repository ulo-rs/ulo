//! Every crate in the workspace is proved somewhere, and every example is
//! reachable from an index — asserted against the tree rather than described.
//!
//! The prose that used to carry this was a hand-maintained mirror of a
//! directory listing, so it drifted: it named seven test files that had been
//! deleted and none of the ninety-eight that replaced them. A listing anything
//! can derive belongs in a test, not a README.
//!
//! What the table below adds that the tree cannot is the decision: a crate
//! proved only by its own `tests/` is a choice, and so is a crate proved
//! nowhere. Each entry is checked in both directions — a crate that gains
//! coverage fails this test as loudly as one that loses it, because a stale
//! "unproved" line is how a hole outlives its fix.
//!
//! This file answers where a crate is proved, never whether what proves it is
//! worth having. That judgment is a per-surface pass, described in
//! `docs/explainers/testing-and-examples.md` along with the rule deciding which
//! of the three homes a test belongs in.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

/// Where a crate's behaviour is proved. Every field is derived from the tree by
/// [`observe`] and compared against the declaration in [`LEDGER`].
#[derive(Debug, PartialEq, Eq)]
struct Proof {
    /// Registered in this crate: a path dependency of `integration-tests`,
    /// exercised from `tests/integration/`. Transports and adapters land here —
    /// their contracts do not exist until an application is running.
    suite: bool,
    /// The crate's own `tests/` directory: behaviour observable without any
    /// other crate present, and the service-backed suites behind `integration`.
    own: bool,
    /// `#[cfg(test)]` modules under `src/`.
    unit: bool,
}

const fn p(suite: bool, own: bool, unit: bool) -> Proof {
    Proof { suite, own, unit }
}

/// Crates that exist to test other crates, and so carry no tests of their own.
/// A support crate is proved by the suites that depend on it: break it, and
/// every transport's conformance run fails at once.
///
/// Checked in both directions like [`HOLES`] — an entry here must have no tests
/// and must be a dev-dependency of something, so a support crate that grows its
/// own suite, or stops being used, fails this test.
const SUPPORT: &[(&str, &str)] = &[(
    "ulo-rpc-conformance",
    "holds the RPC conformance cases the five broker crates instantiate",
)];

/// Reason a crate is proved nowhere. Paired with an entry in [`LEDGER`] whose
/// three fields are all false; `every_hole_is_still_a_hole` rejects a reason
/// attached to a crate that has since gained coverage.
const HOLES: &[(&str, &str)] = &[];

/// One line per crate under `crates/`. A new crate fails `the_ledger_is_complete`
/// until it appears here, which is the point: the decision is made once, in the
/// open, rather than discovered missing a year later.
const LEDGER: &[(&str, Proof)] = &[
    ("ulo", p(true, false, true)),
    ("ulo-build", p(true, false, true)),
    ("ulo-cli", p(false, false, true)),
    ("ulo-config", p(true, true, false)),
    ("ulo-db-diesel", p(false, true, true)),
    ("ulo-db-mongodb", p(false, true, true)),
    ("ulo-db-prisma", p(false, true, false)),
    ("ulo-db-redis", p(false, true, true)),
    ("ulo-db-seaorm", p(false, true, true)),
    ("ulo-db-sqlx", p(false, true, true)),
    ("ulo-graphql-async-graphql", p(true, false, false)),
    ("ulo-graphql-juniper", p(true, false, false)),
    ("ulo-grpc", p(true, false, false)),
    ("ulo-health", p(false, false, true)),
    ("ulo-http-actix", p(true, true, false)),
    ("ulo-http-axum", p(true, true, false)),
    ("ulo-http-poem", p(true, true, false)),
    ("ulo-http-rocket", p(true, true, false)),
    ("ulo-http-salvo", p(true, true, true)),
    ("ulo-macros", p(true, true, true)),
    ("ulo-rpc-kafka", p(false, true, true)),
    ("ulo-rpc-mqtt", p(false, true, true)),
    ("ulo-rpc-nats", p(false, true, false)),
    ("ulo-rpc-rabbitmq", p(false, true, true)),
    ("ulo-rpc-conformance", p(false, false, false)),
    ("ulo-rpc-redis", p(false, true, true)),
    ("ulo-rpc-tcp", p(true, false, true)),
    ("ulo-rpc-udp", p(true, false, true)),
    ("ulo-ws-redis", p(false, true, false)),
    ("ulo-ws-tungstenite", p(true, false, false)),
];

fn workspace_root() -> PathBuf {
    // `integration-tests/` at compile time; its parent is the workspace root.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("integration-tests sits directly under the workspace root")
        .to_path_buf()
}

/// Crate directories under `crates/`, which the workspace globs as `crates/*`.
fn crates_on_disk() -> BTreeSet<String> {
    fs::read_dir(workspace_root().join("crates"))
        .expect("crates/ is readable")
        .map(|entry| entry.expect("directory entry is readable"))
        .filter(|entry| entry.path().join("Cargo.toml").is_file())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect()
}

fn rust_files_in(dir: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .map(|entry| entry.expect("directory entry is readable").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "rs"))
        .map(|path| {
            path.file_stem()
                .expect("a .rs path has a stem")
                .to_string_lossy()
                .into_owned()
        })
        .collect()
}

/// True when any file under `dir` (recursively) contains `needle`.
fn tree_contains(dir: &Path, needle: &str) -> bool {
    let Ok(entries) = fs::read_dir(dir) else {
        return false;
    };
    entries.into_iter().any(|entry| {
        let path = entry.expect("directory entry is readable").path();
        if path.is_dir() {
            tree_contains(&path, needle)
        } else {
            fs::read_to_string(&path).is_ok_and(|text| text.contains(needle))
        }
    })
}

fn observe(krate: &str) -> Proof {
    let root = workspace_root();
    let dir = root.join("crates").join(krate);

    // Path dependencies are declared the same way in every section, so one scan
    // of the manifest text catches `[dependencies]` and `[build-dependencies]`
    // alike — ulo-build is registered as the latter.
    let manifest = fs::read_to_string(root.join("integration-tests/Cargo.toml"))
        .expect("the integration-tests manifest is readable");

    Proof {
        suite: manifest.contains(&format!("\n{krate} = {{ path =")),
        own: !rust_files_in(&dir.join("tests")).is_empty(),
        unit: tree_contains(&dir.join("src"), "#[cfg(test)]"),
    }
}

#[test]
fn the_ledger_is_complete() {
    let declared: BTreeSet<String> = LEDGER.iter().map(|(name, _)| name.to_string()).collect();
    let on_disk = crates_on_disk();

    let unlisted: Vec<_> = on_disk.difference(&declared).collect();
    assert!(
        unlisted.is_empty(),
        "crates with no line in LEDGER: {unlisted:?}\n\
         Add one saying where the crate is proved — `integration-tests` \
         (see its README), its own `tests/`, or neither with a HOLES entry."
    );

    let phantom: Vec<_> = declared.difference(&on_disk).collect();
    assert!(
        phantom.is_empty(),
        "LEDGER names crates that are not under crates/: {phantom:?}"
    );
}

#[test]
fn the_ledger_matches_the_tree() {
    let wrong: Vec<String> = LEDGER
        .iter()
        .filter_map(|(name, declared)| {
            let observed = observe(name);
            (observed != *declared)
                .then(|| format!("  {name}: declared {declared:?}, found {observed:?}"))
        })
        .collect();

    assert!(
        wrong.is_empty(),
        "LEDGER disagrees with the tree:\n{}\n\n\
         `suite` = a path dependency in integration-tests/Cargo.toml; \
         `own` = crates/<name>/tests/*.rs; `unit` = #[cfg(test)] under src/.\n\
         Update the line to match the change.",
        wrong.join("\n")
    );
}

#[test]
fn every_hole_is_still_a_hole() {
    for (name, reason) in HOLES {
        let observed = observe(name);
        assert!(
            !observed.suite && !observed.own && !observed.unit,
            "{name} is listed in HOLES ({reason}) but now has coverage: {observed:?}.\n\
             Drop the HOLES entry and update its LEDGER line."
        );
    }

    let explained: BTreeSet<&str> = HOLES
        .iter()
        .chain(SUPPORT.iter())
        .map(|(name, _)| *name)
        .collect();
    let unexplained: Vec<&str> = LEDGER
        .iter()
        .filter(|(_, proof)| !proof.suite && !proof.own && !proof.unit)
        .map(|(name, _)| *name)
        .filter(|name| !explained.contains(name))
        .collect();

    assert!(
        unexplained.is_empty(),
        "crates proved nowhere, and in neither HOLES nor SUPPORT: {unexplained:?}\n\
         Either prove them, say why they are not proved, or record them as test support."
    );
}

/// A support crate earns its exemption by being used. One that nothing depends
/// on is dead weight wearing an exemption, and one that grew its own tests is
/// no longer a support crate.
#[test]
fn every_support_crate_is_used_and_untested() {
    let root = workspace_root();

    for (name, reason) in SUPPORT {
        let observed = observe(name);
        assert!(
            !observed.suite && !observed.own && !observed.unit,
            "{name} is listed in SUPPORT ({reason}) but now has tests of its own: {observed:?}.\n\
             A crate with its own suite belongs in LEDGER on its own terms."
        );

        let dependents: Vec<String> = crates_on_disk()
            .into_iter()
            .filter(|krate| krate != name)
            .filter(|krate| {
                fs::read_to_string(root.join("crates").join(krate).join("Cargo.toml"))
                    .is_ok_and(|manifest| manifest.contains(&format!("\n{name} = {{ path =")))
            })
            .collect();

        assert!(
            !dependents.is_empty(),
            "{name} is listed in SUPPORT ({reason}) but no crate depends on it."
        );
    }
}

/// A test file nobody declared is compiled by nothing and run by nothing; cargo
/// reports neither, so the file reads as coverage while contributing none.
#[test]
fn every_suite_file_is_declared() {
    let suite = workspace_root().join("integration-tests/tests/integration");
    let main = fs::read_to_string(suite.join("main.rs")).expect("the suite's main.rs is readable");

    let undeclared: Vec<String> = rust_files_in(&suite)
        .into_iter()
        .filter(|name| name != "main")
        .filter(|name| !main.contains(&format!("mod {name};")))
        .collect();

    assert!(
        undeclared.is_empty(),
        "test files with no `mod` line in tests/integration/main.rs: {undeclared:?}\n\
         Without one they never compile and never run."
    );
}

/// A test file's name says which contract it covers; its header says what the
/// contract is. Reviewing a suite this size is otherwise a matter of reading
/// every assertion to recover an intent the author already knew.
#[test]
fn every_suite_file_says_what_it_proves() {
    let suite = workspace_root().join("integration-tests/tests/integration");

    let silent: Vec<String> = rust_files_in(&suite)
        .into_iter()
        .filter(|name| name != "main")
        .filter(|name| {
            let text = fs::read_to_string(suite.join(format!("{name}.rs")))
                .expect("a listed test file is readable");
            !text.starts_with("//!")
        })
        .collect();

    assert!(
        silent.is_empty(),
        "test files that open with no `//!` header: {silent:?}\n\
         State the contract the file exists to prove, not the steps it takes."
    );
}

/// Examples live in two places — the `ulo-examples` crate, and the two GraphQL
/// crates, whose derive macros emit `::async_graphql` / `::juniper` paths that
/// only resolve against a direct dependency. One index covers both.
#[test]
fn every_example_is_indexed() {
    let root = workspace_root();
    let index = fs::read_to_string(root.join("examples/README.md"))
        .expect("the examples index is readable");

    let mut missing = Vec::new();

    for name in rust_files_in(&root.join("examples")) {
        // The proto build script is not an example.
        if name == "build" {
            continue;
        }
        if !index.contains(&name) {
            missing.push(format!("examples/{name}.rs"));
        }
    }

    for krate in crates_on_disk() {
        for name in rust_files_in(&root.join("crates").join(&krate).join("examples")) {
            if !index.contains(&format!("{krate}/examples/{name}.rs")) {
                missing.push(format!("crates/{krate}/examples/{name}.rs"));
            }
        }
    }

    missing.sort();
    assert!(
        missing.is_empty(),
        "examples named nowhere in examples/README.md: {missing:?}\n\
         An example no index reaches is read by nobody."
    );
}

/// `cargo run --example <name>` only works for a name the manifest declares.
#[test]
fn every_example_is_runnable() {
    let root = workspace_root();
    let manifest = fs::read_to_string(root.join("examples/Cargo.toml"))
        .expect("the examples manifest is readable");

    let undeclared: Vec<String> = rust_files_in(&root.join("examples"))
        .into_iter()
        .filter(|name| name != "build")
        .filter(|name| !manifest.contains(&format!("name = \"{name}\"")))
        .collect();

    assert!(
        undeclared.is_empty(),
        "examples with no [[example]] entry in examples/Cargo.toml: {undeclared:?}\n\
         `cargo run --example <name>` cannot reach them."
    );
}
