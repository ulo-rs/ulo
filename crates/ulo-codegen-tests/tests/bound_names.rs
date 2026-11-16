//! A bound is named `max_<thing>` (transports DESIGN, decision 11): no crate source, example,
//! README or design document spells the retired names of the HTTP body bound, its metadata type
//! and the WebSocket message bound, so neither an alias, a macro key nor a doc keeps one
//! reachable. The dated records beside the designs keep the names they were written with.

use std::fs;
use std::path::{Path, PathBuf};

/// The retired names, written in pieces so this file does not spell them. A snake-case name
/// matches inside a longer identifier, `default_` prefixed or a test name; the type matches as a
/// whole word, which leaves tower-http's `RequestBodyLimitLayer` alone.
const SNAKE: &[[&str; 2]] = &[["body", "_limit"], ["message", "_limit"]];
const TYPE: [&str; 2] = ["Body", "Limit"];

/// The design documents that describe the framework as it stands, read beside the crates.
const DESIGNS: &[&str] = &[
    "docs/experiments/di-redesign/DESIGN.md",
    "docs/experiments/di-redesign/CORE_TEST_PLAN.md",
    "docs/experiments/di-redesign/transports/DESIGN.md",
    "docs/experiments/di-redesign/integrations/DESIGN.md",
];

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Every `.rs`, `.md`, `.toml` and `.stderr` file under `dir`, `target` directories skipped.
fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        if path.is_dir() {
            if name != "target" && name != ".git" {
                sources(&path, out);
            }
        } else if path.extension().is_some_and(|ext| ["rs", "md", "toml", "stderr"].iter().any(|e| ext == *e)) {
            out.push(path);
        }
    }
}

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The retired names `text` spells, with the line of each.
fn retired_in(text: &str) -> Vec<(usize, String)> {
    let snake: Vec<String> = SNAKE.iter().map(|parts| parts.concat()).collect();
    let ty = TYPE.concat();
    let mut found = Vec::new();
    for (n, line) in text.lines().enumerate() {
        for name in &snake {
            if line.contains(name.as_str()) {
                found.push((n + 1, name.clone()));
            }
        }
        for (at, _) in line.match_indices(ty.as_str()) {
            let before = line[..at].chars().next_back();
            let after = line[at + ty.len()..].chars().next();
            if !before.is_some_and(is_ident) && !after.is_some_and(is_ident) {
                found.push((n + 1, ty.clone()));
            }
        }
    }
    found
}

#[test]
fn the_matcher_finds_each_retired_name_and_passes_the_tower_layer() {
    let planted = format!(
        "server.{}(1)\nlet default_{} = 2;\n#[meta({}(8))]\nRequestBodyLimitLayer::new(1)\n",
        SNAKE[0].concat(),
        SNAKE[1].concat(),
        TYPE.concat(),
    );
    let found: Vec<usize> = retired_in(&planted).into_iter().map(|(line, _)| line).collect();
    assert_eq!(found, [1, 2, 3], "the plants on lines 1-3 and not the tower layer on line 4");
}

#[test]
fn no_crate_example_readme_or_design_spells_a_retired_bound_name() {
    let root = root();
    let mut files = Vec::new();
    sources(&root.join("crates"), &mut files);
    sources(&root.join("examples"), &mut files);
    files.push(root.join("README.md"));
    for design in DESIGNS {
        let path = root.join(design);
        assert!(path.is_file(), "{design} is not a file; `DESIGNS` names the designs read");
        files.push(path);
    }
    let this = Path::new(file!()).file_name().expect("a file name");
    let mut hits = Vec::new();
    let mut read = 0;
    for path in &files {
        if path.file_name() == Some(this) {
            continue;
        }
        let Ok(text) = fs::read_to_string(path) else { continue };
        read += 1;
        for (line, name) in retired_in(&text) {
            hits.push(format!("{}:{line}: `{name}`", path.strip_prefix(&root).unwrap_or(path).display()));
        }
    }
    assert!(read > 500, "read {read} files from {}; the walk missed the tree", root.display());
    assert!(hits.is_empty(), "retired bound names, spelled `max_body`, `max_message` and `MaxBody`:\n{}", hits.join("\n"));
}
