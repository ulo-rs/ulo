use serde::Deserialize;
use ulo::Secret;
use ulo_config::Problems;

/// An engine SQLx speaks, for the scheme check `ulo-db-seaorm` shares.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Engine {
    Postgres,
    MySql,
    Sqlite,
}

impl Engine {
    /// The URL schemes SQLx's option parser accepts for the engine, without the `:` or `://`.
    pub fn schemes(self) -> &'static [&'static str] {
        todo!()
    }
}

/// Checks that `url` starts with a scheme `engine` takes. SQLx's option parsers accept any scheme
/// (`notascheme://` parses), so this runs first. The problem names the schemes expected and prints
/// no part of the URL.
pub fn check_scheme(engine: Engine, url: &str) -> Result<(), Problems> {
    let _ = (engine, url);
    todo!()
}

/// [`check_scheme`], then the engine's own `*ConnectOptions::from_str`. A failure is a problem at
/// `url` written from its category with the userinfo replaced by the redaction marker, never from
/// text that may echo the URL. No malformed URL reaches a driver, which is how `ulo-db-seaorm`
/// keeps SeaORM's `Database::connect`, whose errors print the whole URL, from seeing one.
pub fn validate_url(engine: Engine, url: &str) -> Result<(), Problems> {
    let _ = (engine, url);
    todo!()
}

/// Postgres options. `Debug` prints the URL redacted, and no library options value is formatted.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Deserialize)]
pub struct PgOptions {
    /// `postgres://user:password@host/database`, libpq's connection URI.
    pub url: Secret<String>,
    pub max_connections: Option<u32>,
    pub min_connections: Option<u32>,
    pub acquire_timeout_secs: Option<u64>,
}

/// MySQL options; the `mysql://` scheme is a convention and not a standard.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Deserialize)]
pub struct MySqlOptions {
    pub url: Secret<String>,
    pub max_connections: Option<u32>,
    pub min_connections: Option<u32>,
    pub acquire_timeout_secs: Option<u64>,
}

/// SQLite options; `sqlite:` followed by a file path, or `sqlite::memory:`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Deserialize)]
pub struct SqliteOptions {
    pub url: Secret<String>,
    pub max_connections: Option<u32>,
    pub min_connections: Option<u32>,
    pub acquire_timeout_secs: Option<u64>,
}
