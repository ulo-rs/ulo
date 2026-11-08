//! Startup failures written to files: a scenario whose environment did not start fails through
//! [`startup_failed!`](macro@crate::startup_failed), which writes the message, with its full error chain,
//! to a file of its own before panicking with it, so the cause survives a runner that keeps only
//! part of the test output. CI uploads the directory when a job fails.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Where startup failures are written: `$ULO_CONFORMANCE_FAILURES` when it is set, otherwise
/// `conformance-failures` in the target directory the running test binary was built into, read
/// off the binary's path, `<target>/<profile>/deps/<binary>`: `target/conformance-failures` at the
/// root of a workspace built into its default target directory. A binary whose own directory is
/// not a `deps` directory writes beside itself.
pub fn failures_dir() -> PathBuf {
    match std::env::var_os("ULO_CONFORMANCE_FAILURES") {
        Some(dir) => PathBuf::from(dir),
        None => match std::env::current_exe() {
            Ok(binary) => target_dir(&binary).join("conformance-failures"),
            Err(_) => std::env::temp_dir().join("conformance-failures"),
        },
    }
}

/// The directory two above `binary`'s own when that is a `deps` directory, otherwise the binary's
/// own directory. Only the binary's own directory is read: a `deps` further up is some other
/// directory, such as one holding the checkout.
fn target_dir(binary: &Path) -> PathBuf {
    let dir = binary.parent().unwrap_or(binary);
    match dir.file_name() {
        Some(name) if name == "deps" => dir.parent().and_then(Path::parent).unwrap_or(dir).to_path_buf(),
        _ => dir.to_path_buf(),
    }
}

/// The function [`startup_failed!`](macro@crate::startup_failed) expands to: writes `message`
/// under [`failures_dir`], then panics with it and the file's path, at the caller's location.
#[track_caller]
pub fn startup_failed(message: String) -> ! {
    match write(&message) {
        Ok(path) => panic!("{message}\n(written to {})", path.display()),
        Err(error) => panic!("{message}\n(not written under {}: {error})", failures_dir().display()),
    }
}

/// One file per failure, named after the package under test, the test (libtest names the thread
/// running it after the test), the process and the time.
fn write(message: &str) -> io::Result<PathBuf> {
    let dir = failures_dir();
    fs::create_dir_all(&dir)?;
    let binary = std::env::current_exe().ok();
    let suite = std::env::var("CARGO_PKG_NAME").ok().or_else(|| {
        binary.as_ref().and_then(|exe| exe.file_stem()).map(|stem| stem.to_string_lossy().into_owned())
    });
    let suite = suite.unwrap_or_else(|| "unknown".to_owned());
    let thread = std::thread::current();
    let test = thread.name().unwrap_or("unnamed");
    let since = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let name: String = format!("{suite}-{test}-{}-{}.txt", std::process::id(), since.as_nanos())
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') { c } else { '_' })
        .collect();
    let path = dir.join(name);
    let binary = binary.map_or_else(String::new, |exe| exe.display().to_string());
    fs::write(&path, format!("package: {suite}\ntest: {test}\nbinary: {binary}\nunix time: {}\n\n{message}\n", since.as_secs()))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::target_dir;

    #[test]
    fn a_test_binary_writes_to_its_target_directory() {
        let binary = Path::new("/w/target/debug/deps/conformance-0123abcd");
        assert_eq!(target_dir(binary), PathBuf::from("/w/target"));
    }

    #[test]
    fn a_binary_built_for_a_named_target_writes_below_its_triple() {
        let binary = Path::new("/w/target/x86_64-unknown-linux-gnu/release/deps/conformance-0123abcd");
        assert_eq!(target_dir(binary), PathBuf::from("/w/target/x86_64-unknown-linux-gnu"));
    }

    #[test]
    fn a_binary_in_no_deps_directory_writes_beside_itself() {
        let binary = Path::new("/opt/suite/bin/conformance");
        assert_eq!(target_dir(binary), PathBuf::from("/opt/suite/bin"));
    }

    #[test]
    fn a_deps_directory_above_the_binary_s_own_is_not_its_target() {
        let binary = Path::new("/home/deps/w/target/debug/examples/conformance-0123abcd");
        assert_eq!(target_dir(binary), PathBuf::from("/home/deps/w/target/debug/examples"));
    }
}
