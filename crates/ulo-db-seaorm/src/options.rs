use serde::Deserialize;
use ulo::Secret;
use ulo_config::Problems;

/// SeaORM options. `Debug` prints the URL redacted, and no `ConnectOptions` value is formatted.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Deserialize)]
pub struct SeaOrmOptions {
    /// A `postgres://`, `mysql://` or `sqlite:` URL.
    pub url: Secret<String>,
    pub max_connections: Option<u32>,
    pub min_connections: Option<u32>,
    pub acquire_timeout_secs: Option<u64>,
}

/// The scheme check `ulo-db-sqlx` shares, then SQLx's option parser for the engine the scheme names.
pub(crate) fn validate(o: &SeaOrmOptions) -> Result<(), Problems> {
    let _ = o;
    todo!()
}
