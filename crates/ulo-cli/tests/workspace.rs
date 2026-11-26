//! `ulo new` below a Cargo workspace yields a project cargo accepts.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// A fresh directory for one test. It sits outside the repository, whose own workspace root would
/// otherwise be the nearest one above every project.
fn scratch(name: &str) -> PathBuf {
    let base = std::env::temp_dir().join(format!("ulo-cli-workspace-{}", std::process::id()));
    // A manifest above the temporary directory, the repository's when TMPDIR points inside it, would
    // be the root these cases edit.
    if let Some(manifest) = base.ancestors().map(|dir| dir.join("Cargo.toml")).find(|path| path.exists()) {
        panic!("{} lies above the temporary directory; point TMPDIR outside every Cargo project", manifest.display());
    }
    let dir = base.join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("a scratch directory");
    dir
}

fn ulo_new(cwd: &Path, name: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ulo")).args(["new", name]).current_dir(cwd).output().expect("ulo runs")
}

/// `cargo metadata` on the project's manifest, which is where cargo reports a package that
/// believes it is in a workspace when it is not.
fn metadata(project: &Path) -> Output {
    Command::new(env!("CARGO"))
        .args(["metadata", "--no-deps", "--format-version", "1", "--manifest-path"])
        .arg(project.join("Cargo.toml"))
        .output()
        .expect("cargo runs")
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn created(cwd: &Path, name: &str) -> Output {
    let output = ulo_new(cwd, name);
    assert!(output.status.success(), "{}", stderr(&output));
    let metadata = metadata(&cwd.join(name));
    assert!(metadata.status.success(), "{}", stderr(&metadata));
    output
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn a_root_with_an_explicit_members_list_gains_the_project() {
    let root = scratch("explicit");
    let manifest = "# the root\n[workspace]\nmembers = [ \"existing\" ]  # kept\n";
    fs::write(root.join("Cargo.toml"), manifest).expect("a root");
    fs::create_dir_all(root.join("existing/src")).expect("a member");
    fs::write(root.join("existing/Cargo.toml"), "[package]\nname = \"existing\"\nversion = \"0.1.0\"\nedition = \"2024\"\n")
        .expect("a member manifest");
    fs::write(root.join("existing/src/lib.rs"), "").expect("a member source");

    let output = created(&root, "app");
    assert!(stdout(&output).contains("Added `app`"), "{}", stdout(&output));
    let text = fs::read_to_string(root.join("Cargo.toml")).expect("the root");
    // `cargo new` writes these bytes for the same manifest: a sorted list stays sorted.
    assert_eq!(text, "# the root\n[workspace]\nmembers = [\"app\", \"existing\" ]  # kept\n");
    assert!(!fs::read_to_string(root.join("app/Cargo.toml")).expect("the project").contains("[workspace]"));
}

#[test]
fn a_glob_that_matches_leaves_the_root_alone() {
    let root = scratch("glob");
    let manifest = "[workspace]\nmembers = [\"crates/*\"]\n";
    fs::write(root.join("Cargo.toml"), manifest).expect("a root");
    let crates = root.join("crates");
    fs::create_dir_all(&crates).expect("a crates directory");

    let output = created(&crates, "app");
    assert!(!stdout(&output).contains("Added"), "{}", stdout(&output));
    assert_eq!(fs::read_to_string(root.join("Cargo.toml")).expect("the root"), manifest);
}

#[test]
fn a_virtual_workspace_gains_a_nested_project_by_its_relative_path() {
    let root = scratch("virtual");
    fs::write(root.join("Cargo.toml"), "[workspace]\nresolver = \"3\"\nmembers = [\"crates/*\"]\n").expect("a root");
    let apps = root.join("apps/web");
    fs::create_dir_all(&apps).expect("a directory");
    // A glob that expands to nothing is read by cargo as a literal path, which does not exist.
    fs::create_dir_all(root.join("crates/lib/src")).expect("a member");
    fs::write(root.join("crates/lib/Cargo.toml"), "[package]\nname = \"lib\"\nversion = \"0.1.0\"\nedition = \"2024\"\n")
        .expect("a member manifest");
    fs::write(root.join("crates/lib/src/lib.rs"), "").expect("a member source");

    created(&apps, "app");
    let text = fs::read_to_string(root.join("Cargo.toml")).expect("the root");
    assert!(text.contains("\"apps/web/app\""), "{text}");
}

#[test]
fn an_ancestor_without_a_workspace_table_is_not_a_root() {
    let root = scratch("plain");
    let manifest = "[package]\nname = \"outer\"\nversion = \"0.1.0\"\nedition = \"2024\"\n";
    fs::write(root.join("Cargo.toml"), manifest).expect("a plain parent");
    fs::create_dir_all(root.join("src")).expect("a source directory");
    fs::write(root.join("src/lib.rs"), "").expect("a source");

    let output = created(&root, "app");
    assert!(!stdout(&output).contains("Added"), "{}", stdout(&output));
    assert_eq!(fs::read_to_string(root.join("Cargo.toml")).expect("the parent"), manifest);
}

#[test]
fn the_nearest_workspace_root_wins_past_a_plain_manifest() {
    let root = scratch("nearest");
    fs::write(root.join("Cargo.toml"), "[workspace]\nmembers = []\n").expect("a root");
    let inner = root.join("inner");
    fs::create_dir_all(&inner).expect("a directory");
    fs::write(inner.join("Cargo.toml"), "[package]\nname = \"inner\"\nversion = \"0.1.0\"\nedition = \"2024\"\n").expect("a plain manifest");
    fs::create_dir_all(inner.join("src")).expect("a source directory");
    fs::write(inner.join("src/lib.rs"), "").expect("a source");

    created(&inner, "app");
    let text = fs::read_to_string(root.join("Cargo.toml")).expect("the root");
    assert!(text.contains("\"inner/app\""), "{text}");
}

#[test]
fn a_root_that_cannot_take_a_member_makes_the_project_its_own_workspace() {
    let root = scratch("refused");
    fs::write(root.join("Cargo.toml"), "[workspace]\nmembers = \"nope\"\n").expect("a root");

    let output = ulo_new(&root, "app");
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(fs::read_to_string(root.join("app/Cargo.toml")).expect("the project").contains("\n[workspace]\n"));
    assert_eq!(fs::read_to_string(root.join("Cargo.toml")).expect("the root"), "[workspace]\nmembers = \"nope\"\n");
}

#[test]
fn a_manifest_above_that_does_not_parse_makes_the_project_its_own_workspace() {
    let root = scratch("unparsed");
    fs::write(root.join("Cargo.toml"), "[package\n").expect("a broken manifest");

    let output = created(&root, "app");
    assert!(stdout(&output).contains("it does not parse"), "{}", stdout(&output));
    assert_eq!(fs::read_to_string(root.join("Cargo.toml")).expect("the manifest"), "[package\n");
}

#[test]
fn an_excluded_directory_is_left_out_of_the_workspace() {
    let root = scratch("excluded");
    let manifest = "[workspace]\nmembers = [\"crates/*\"]\nexclude = [\"scratch\"]\n";
    fs::write(root.join("Cargo.toml"), manifest).expect("a root");
    let scratch_dir = root.join("scratch");
    fs::create_dir_all(&scratch_dir).expect("a directory");

    created(&scratch_dir, "app");
    assert_eq!(fs::read_to_string(root.join("Cargo.toml")).expect("the root"), manifest);
}
