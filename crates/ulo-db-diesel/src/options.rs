use serde::Deserialize;
use ulo::Secret;
use ulo_config::Problems;

/// Options for every engine: the URL, which `Debug` prints redacted, and the pool's size and wait.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Deserialize)]
pub struct DieselOptions {
    /// `postgres://`, `mysql://` or a SQLite path.
    pub url: Secret<String>,
    pub max_size: Option<usize>,
    pub wait_timeout_secs: Option<u64>,
}

/// Checks the URL's scheme against `schemes`; the problem names the schemes expected and prints no
/// part of the URL.
pub(crate) fn check_scheme(o: &DieselOptions, schemes: &[&str]) -> Result<(), Problems> {
    let _ = (o, schemes);
    todo!()
}
