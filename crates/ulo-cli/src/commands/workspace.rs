//! Placing a new project inside the Cargo workspace it is created under.
//!
//! A manifest with no `[workspace]` table, below one that has it, is refused by cargo ("current
//! package believes it's in a workspace when it's not") unless the workspace lists the package.
//! `cargo new` adds the member to the nearest ancestor workspace root; this does the same, and
//! leaves the rest of that manifest's text as it was.

use std::fs::{self, OpenOptions, Permissions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use glob::{MatchOptions, Pattern};
use toml_edit::{Array, DocumentMut, Item, Value};

/// How a project relates to the workspace above it.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Placement {
    /// No ancestor manifest has a `[workspace]` table.
    Outside,
    /// The workspace's `members` already match the project, or its `exclude` leaves it out.
    Covered,
    /// The workspace root's manifest needs `text` for the project to be a member.
    Member { manifest: PathBuf, text: String, member: String },
    /// The workspace root cannot be edited to take the project; it carries its own `[workspace]`.
    Standalone { manifest: PathBuf, reason: String },
}

/// The nearest ancestor of `project` whose `Cargo.toml` has a `[workspace]` table, as cargo finds
/// it: a `Cargo.toml` without one is not a workspace root and the walk goes on past it.
pub(crate) fn place(project: &Path) -> Placement {
    for dir in project.ancestors().skip(1) {
        let manifest = dir.join("Cargo.toml");
        let Ok(text) = std::fs::read_to_string(&manifest) else { continue };
        let Ok(mut doc) = text.parse::<DocumentMut>() else {
            // Cargo stops its search with an error at a manifest that does not parse, unless the
            // package below it is a workspace root of its own.
            return Placement::Standalone { manifest, reason: "it does not parse".to_owned() };
        };
        if !doc.contains_key("workspace") {
            continue;
        }
        let Ok(relative) = project.strip_prefix(dir) else { continue };
        let member = relative.components().map(|part| part.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/");
        return match add_member(&mut doc, &member) {
            Ok(false) => Placement::Covered,
            Ok(true) => Placement::Member { manifest, text: doc.to_string(), member },
            Err(reason) => Placement::Standalone { manifest, reason },
        };
    }
    Placement::Outside
}

/// Adds `member` to `workspace.members` unless the table already covers it. `Ok(true)` when the
/// document changed.
fn add_member(doc: &mut DocumentMut, member: &str) -> Result<bool, String> {
    let workspace =
        doc.get_mut("workspace").and_then(Item::as_table_like_mut).ok_or("`workspace` is not a table")?;

    if let Some(exclude) = workspace.get("exclude") {
        let exclude = exclude.as_array().ok_or("`workspace.exclude` is not an array")?;
        // An excluded path takes its whole subtree out of the workspace.
        let excluded = |path: &str| Path::new(member).starts_with(path.trim_start_matches("./"));
        if exclude.iter().filter_map(Value::as_str).any(excluded) {
            return Ok(false);
        }
    }

    match workspace.get_mut("members") {
        None => {
            let mut members = Array::new();
            members.push(member);
            workspace.insert("members", Item::Value(Value::Array(members)));
            Ok(true)
        }
        Some(members) => {
            let members = members.as_array_mut().ok_or("`workspace.members` is not an array")?;
            if members.iter().filter_map(Value::as_str).any(|pattern| matches(pattern, member)) {
                return Ok(false);
            }
            let sorted = members.iter().map(Value::as_str).is_sorted();
            members.push(member);
            // A sorted list stays sorted, as `cargo new` keeps it.
            if sorted {
                members.sort_by(|left, right| left.as_str().cmp(&right.as_str()));
            }
            Ok(true)
        }
    }
}

/// Whether a `workspace.members` entry, a path or a glob, names `member`. A `*` does not cross a
/// separator, as in the expansion cargo performs on disk.
fn matches(pattern: &str, member: &str) -> bool {
    let pattern = pattern.trim_start_matches("./").trim_end_matches('/');
    let options = MatchOptions { case_sensitive: true, require_literal_separator: true, require_literal_leading_dot: false };
    pattern == member || Pattern::new(pattern).is_ok_and(|pattern| pattern.matches_with(member, options))
}

/// Replaces the manifest at `path` with `text` through a sibling file renamed over it, as cargo
/// writes one, so a write that fails or is interrupted leaves the original whole. A symlink is
/// followed and kept, the permissions carry over, and a file this process may not write is refused
/// rather than replaced.
pub(crate) fn write(path: &Path, text: &str) -> io::Result<()> {
    let target = fs::canonicalize(path)?;
    // Opening for writing without truncating is refused exactly where a plain write would be.
    let permissions = OpenOptions::new().write(true).open(&target)?.metadata()?.permissions();
    let name = target.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let staged = target.with_file_name(format!(".{name}.ulo-new-{}", std::process::id()));
    let result = stage(&staged, text, permissions).and_then(|()| fs::rename(&staged, &target));
    if result.is_err() {
        let _ = fs::remove_file(&staged);
    }
    result
}

fn stage(path: &Path, text: &str, permissions: Permissions) -> io::Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(text.as_bytes())?;
    file.set_permissions(permissions)?;
    file.sync_all()
}

#[cfg(test)]
mod tests {
    use super::{DocumentMut, add_member, write};

    fn edit(manifest: &str, member: &str) -> (Result<bool, String>, String) {
        let mut doc: DocumentMut = manifest.parse().expect("a manifest");
        let changed = add_member(&mut doc, member);
        (changed, doc.to_string())
    }

    #[test]
    fn a_member_is_appended_and_the_rest_of_the_text_is_kept() {
        let (changed, text) = edit("# root\n[workspace]\nmembers = [ \"a\" ]  # keep\n", "sub/b");
        assert_eq!(changed, Ok(true));
        assert!(text.starts_with("# root\n[workspace]\nmembers = [ \"a\" , \"sub/b\"]  # keep"), "{text}");
    }

    #[test]
    fn a_workspace_with_no_members_gains_the_key() {
        let (changed, text) = edit("[package]\nname = \"r\"\n[workspace]\n", "x");
        assert_eq!(changed, Ok(true));
        assert!(text.contains("members = [\"x\"]"), "{text}");
    }

    #[test]
    fn a_covering_glob_or_path_and_an_exclusion_leave_the_manifest_alone() {
        for (manifest, member) in [
            ("[workspace]\nmembers = [\"crates/*\"]\n", "crates/app"),
            ("[workspace]\nmembers = [\"./app/\"]\n", "app"),
            ("[workspace]\nmembers = [\"**\"]\n", "a/b"),
            ("[workspace]\nmembers = [\"a\"]\nexclude = [\"crates\"]\n", "crates/app"),
            ("[workspace]\nmembers = [\"a\"]\nexclude = [\"./crates/\"]\n", "crates/app"),
        ] {
            let (changed, text) = edit(manifest, member);
            assert_eq!(changed, Ok(false), "{manifest}");
            assert_eq!(text, manifest);
        }
    }

    #[test]
    fn a_star_does_not_reach_a_deeper_path() {
        let (changed, _) = edit("[workspace]\nmembers = [\"crates/*\"]\n", "crates/a/b");
        assert_eq!(changed, Ok(true));
    }

    #[test]
    fn a_sorted_list_stays_sorted_and_an_unsorted_one_is_appended_to() {
        assert_eq!(edit("[workspace]\nmembers = [\"a\", \"c\"]\n", "b").1, "[workspace]\nmembers = [\"a\", \"b\", \"c\"]\n");
        assert_eq!(edit("[workspace]\nmembers = [\"c\", \"a\"]\n", "b").1, "[workspace]\nmembers = [\"c\", \"a\", \"b\"]\n");
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_manifest_keeps_its_link_and_its_target_keeps_its_mode() {
        use std::os::unix::fs::{PermissionsExt, symlink};

        let dir = std::env::temp_dir().join(format!("ulo-cli-write-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("real")).expect("a directory");
        let target = dir.join("real/Cargo.toml");
        std::fs::write(&target, "old").expect("a manifest");
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o640)).expect("a mode");
        let link = dir.join("Cargo.toml");
        symlink(&target, &link).expect("a link");

        write(&link, "new").expect("the write");
        assert!(std::fs::symlink_metadata(&link).expect("the link").file_type().is_symlink());
        assert_eq!(std::fs::read_to_string(&target).expect("the target"), "new");
        assert_eq!(std::fs::metadata(&target).expect("the target").permissions().mode() & 0o777, 0o640);
        assert_eq!(std::fs::read_dir(dir.join("real")).expect("the directory").count(), 1, "the staged file is gone");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_read_only_manifest_is_refused_and_left_alone() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!("ulo-cli-readonly-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a directory");
        let manifest = dir.join("Cargo.toml");
        std::fs::write(&manifest, "old").expect("a manifest");
        std::fs::set_permissions(&manifest, std::fs::Permissions::from_mode(0o444)).expect("a mode");

        assert!(write(&manifest, "new").is_err());
        assert_eq!(std::fs::read_to_string(&manifest).expect("the manifest"), "old");
        assert_eq!(std::fs::read_dir(&dir).expect("the directory").count(), 1, "no staged file is left");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn members_that_is_not_an_array_is_refused() {
        assert!(edit("[workspace]\nmembers = \"a\"\n", "b").0.is_err());
    }
}
