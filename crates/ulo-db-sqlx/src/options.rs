use serde::Deserialize;
use ulo::Secret;
use ulo_config::Problems;

/// The wait for a pooled connection when `acquire_timeout_secs` is not set. SQLx's own default is
/// 30 s, which is also the default total connect bound, so one attempt of a refused connection
/// would take the whole bound and `.retries(..)` would never get a second attempt.
#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
pub(crate) const DEFAULT_ACQUIRE_SECS: u64 = 5;

/// Pool size SQLx uses when `max_connections` is not set; the bound `min_connections` is checked
/// against.
#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
const DEFAULT_MAX_CONNECTIONS: u32 = 10;

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
        match self {
            Engine::Postgres => &["postgres", "postgresql"],
            Engine::MySql => &["mysql", "mariadb"],
            Engine::Sqlite => &["sqlite"],
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Engine::Postgres => "postgres",
            Engine::MySql => "mysql",
            Engine::Sqlite => "sqlite",
        }
    }
}

/// Checks that `url` starts with a scheme `engine` takes. SQLx's option parsers accept any scheme
/// (`notascheme://` parses), so this runs first. The problem names the schemes expected and prints
/// no part of the URL.
pub fn check_scheme(engine: Engine, url: &str) -> Result<(), Problems> {
    let accepted = url.split_once(':').is_some_and(|(scheme, rest)| {
        let well_formed = !scheme.is_empty()
            && scheme.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.'));
        // SQLite's form is `sqlite:path` or `sqlite::memory:`; the others are `scheme://authority`.
        let shaped = engine == Engine::Sqlite || rest.starts_with("//");
        well_formed && shaped && engine.schemes().iter().any(|s| s.eq_ignore_ascii_case(scheme))
    });
    if accepted {
        return Ok(());
    }
    let separator = if engine == Engine::Sqlite { ":" } else { "://" };
    let expected: Vec<String> = engine.schemes().iter().map(|s| format!("`{s}{separator}`")).collect();
    let mut problems = Problems::new();
    problems.at("url", format!("expected a {} URL starting with {}", engine.name(), expected.join(" or ")));
    Err(problems)
}

/// [`check_scheme`], then the engine's own `*ConnectOptions::from_str`. A failure is a problem at
/// `url` written from its category, never from text that may echo the URL, and the URL is not
/// printed at all. No malformed URL reaches a driver, which is how `ulo-db-seaorm`
/// keeps SeaORM's `Database::connect`, whose errors print the whole URL, from seeing one.
pub fn validate_url(engine: Engine, url: &str) -> Result<(), Problems> {
    check_scheme(engine, url)?;
    match parse(engine, url) {
        Ok(()) => Ok(()),
        Err(failure) => {
            let mut problems = Problems::new();
            problems.at("url", describe(engine, url, &failure));
            Err(problems)
        }
    }
}

/// Why a URL did not parse. A variant of this crate's own, so that [`describe`] is the one place
/// a library error becomes text. Which variants are built depends on the engine features.
#[allow(dead_code)]
enum Failure {
    Library(sqlx::Error),
    NotCompiled,
}

// With no engine feature every arm is `NotCompiled` and the URL is unread.
#[cfg_attr(not(any(feature = "postgres", feature = "mysql", feature = "sqlite")), allow(unused_variables))]
fn parse(engine: Engine, url: &str) -> Result<(), Failure> {
    match engine {
        #[cfg(feature = "postgres")]
        Engine::Postgres => url.parse::<sqlx::postgres::PgConnectOptions>().map(drop).map_err(Failure::Library),
        #[cfg(not(feature = "postgres"))]
        Engine::Postgres => Err(Failure::NotCompiled),
        #[cfg(feature = "mysql")]
        Engine::MySql => url.parse::<sqlx::mysql::MySqlConnectOptions>().map(drop).map_err(Failure::Library),
        #[cfg(not(feature = "mysql"))]
        Engine::MySql => Err(Failure::NotCompiled),
        #[cfg(feature = "sqlite")]
        Engine::Sqlite => url.parse::<sqlx::sqlite::SqliteConnectOptions>().map(drop).map_err(Failure::Library),
        #[cfg(not(feature = "sqlite"))]
        Engine::Sqlite => Err(Failure::NotCompiled),
    }
}

/// The problem text for a failed parse. The URL itself is never printed. SQLx's configuration
/// errors name a category such as `invalid port number` and, probed against 0.9.0, never carry the
/// URL or its password; the detail is kept only while it contains none of the URL's sensitive
/// pieces, in case a future parser quotes its input.
fn describe(engine: Engine, url: &str, failure: &Failure) -> String {
    let name = engine.name();
    match failure {
        Failure::NotCompiled => format!("the {name} engine is not compiled in; enable this crate's `{name}` feature"),
        Failure::Library(sqlx::Error::Configuration(source)) => {
            let detail = source.to_string();
            if leaks(url, &detail) {
                format!("is not a valid {name} connection URL")
            } else {
                format!("is not a valid {name} connection URL: {detail}")
            }
        }
        Failure::Library(_) => format!("could not be read as a {name} connection URL"),
    }
}

/// Whether `text` contains the URL, its userinfo or a password in any spelling.
fn leaks(url: &str, text: &str) -> bool {
    let mut pieces = password_forms(url);
    if let Some(userinfo) = userinfo(url) {
        pieces.push(userinfo.to_owned());
    }
    pieces.push(url.to_owned());
    pieces.iter().any(|p| !p.is_empty() && text.contains(p.as_str()))
}

/// The userinfo of a `scheme://userinfo@host` URL: everything before the last `@` of the authority.
fn userinfo(url: &str) -> Option<&str> {
    let (_, rest) = url.split_once("://")?;
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (userinfo, _) = rest[..end].rsplit_once('@')?;
    Some(userinfo)
}

/// The password as written and percent-decoded, from the userinfo and from a `password=` query
/// parameter, which libpq's URI form allows. A library error prints the decoded form; the URL
/// holds the written one.
fn password_forms(url: &str) -> Vec<String> {
    let mut forms = Vec::new();
    if let Some((_, password)) = userinfo(url).and_then(|u| u.split_once(':')) {
        forms.push(password.to_owned());
        forms.push(percent_decode(password));
    }
    let query = url.split_once('?').map(|(_, q)| q.split('#').next().unwrap_or(q)).unwrap_or("");
    for pair in query.split('&') {
        if let Some((key, value)) = pair.split_once('=')
            && key.eq_ignore_ascii_case("password")
        {
            forms.push(value.to_owned());
            forms.push(percent_decode(&value.replace('+', " ")));
        }
    }
    forms.retain(|f| !f.is_empty());
    forms.sort();
    forms.dedup();
    forms
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(high), Some(low)) = (char::from(bytes[i + 1]).to_digit(16), char::from(bytes[i + 2]).to_digit(16))
        {
            out.push((high * 16 + low) as u8);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The password component of `url` in each spelling a library error might print, for
/// `Store::secrets`. The whole URL is registered by the section read, as every `Secret` leaf is.
#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
pub(crate) fn password_secrets(url: &str) -> Vec<Secret<String>> {
    password_forms(url).into_iter().map(Secret::new).collect()
}

/// The pool configuration shared by the three engines: SQLx's defaults, except for the acquire
/// timeout (see [`DEFAULT_ACQUIRE_SECS`]).
#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
pub(crate) fn pool_options<DB: sqlx::Database>(
    max_connections: Option<u32>,
    min_connections: Option<u32>,
    acquire_timeout_secs: Option<u64>,
) -> sqlx::pool::PoolOptions<DB> {
    let acquire = std::time::Duration::from_secs(acquire_timeout_secs.unwrap_or(DEFAULT_ACQUIRE_SECS));
    let mut options = sqlx::pool::PoolOptions::<DB>::new().acquire_timeout(acquire);
    if let Some(max) = max_connections {
        options = options.max_connections(max);
    }
    if let Some(min) = min_connections {
        options = options.min_connections(min);
    }
    options
}

/// The URL check plus the pool numbers, shared by the three stores' `validate`.
#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
pub(crate) fn validate_options(
    engine: Engine,
    url: &Secret<String>,
    max_connections: Option<u32>,
    min_connections: Option<u32>,
    acquire_timeout_secs: Option<u64>,
) -> Result<(), Problems> {
    let mut problems = validate_url(engine, url.expose()).err().unwrap_or_default();
    if max_connections == Some(0) {
        problems.at("max_connections", "must be at least 1");
    }
    if let Some(min) = min_connections
        && min > max_connections.unwrap_or(DEFAULT_MAX_CONNECTIONS)
    {
        problems.at("min_connections", "must not exceed max_connections");
    }
    if acquire_timeout_secs == Some(0) {
        problems.at("acquire_timeout_secs", "must be at least 1");
    }
    problems.into_result()
}

/// The error for a URL that passed `validate` and still fails to parse in `create`. Permanent, and
/// written from the same text as the problem.
#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
pub(crate) fn unreadable_url(engine: Engine, url: &str, error: sqlx::Error) -> ulo::BoxError {
    ulo::Permanent::new(format!("url {}", describe(engine, url, &Failure::Library(error)))).into()
}

/// The migration lock of Postgres and MySQL pins one pooled connection while SQLx's migrator takes
/// another, so a pool of one would wait on itself until the acquire timeout.
#[cfg(any(feature = "postgres", feature = "mysql"))]
pub(crate) fn require_spare_connection(engine: Engine, max_connections: u32) -> Result<(), ulo::BoxError> {
    if max_connections >= 2 {
        return Ok(());
    }
    Err(format!(
        "the {} migration lock holds one connection while the migrator takes another, so max_connections must be at least 2",
        engine.name()
    )
    .into())
}

/// Postgres options. `Debug` prints the URL redacted, and no library options value is formatted.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Deserialize)]
pub struct PgOptions {
    /// `postgres://user:password@host/database`, libpq's connection URI.
    pub url: Secret<String>,
    /// Defaults to SQLx's 10.
    pub max_connections: Option<u32>,
    /// Defaults to 0; the pool is lazy, so a minimum is filled in the background and never delays
    /// the start.
    pub min_connections: Option<u32>,
    /// The wait for a pooled connection, 5 s by default. A refused connection reaches the caller
    /// only as `PoolTimedOut` after this long, so it is also the length of one `.ready` attempt
    /// against an unreachable server.
    pub acquire_timeout_secs: Option<u64>,
}

/// MySQL options; the `mysql://` scheme is a convention and not a standard.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Deserialize)]
pub struct MySqlOptions {
    pub url: Secret<String>,
    /// Defaults to SQLx's 10.
    pub max_connections: Option<u32>,
    /// Defaults to 0.
    pub min_connections: Option<u32>,
    /// The wait for a pooled connection, 5 s by default; see [`PgOptions::acquire_timeout_secs`].
    pub acquire_timeout_secs: Option<u64>,
}

/// SQLite options; `sqlite:` followed by a file path, or `sqlite::memory:`.
///
/// SQLx opens a file read-write and does not create it: `sqlite:data.db?mode=rwc` creates a missing
/// file, and a missing one without `rwc` fails the first ping with SQLite's `CANTOPEN` (code 14),
/// which is permanent.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Deserialize)]
pub struct SqliteOptions {
    pub url: Secret<String>,
    /// Defaults to SQLx's 10.
    pub max_connections: Option<u32>,
    /// Defaults to 0.
    pub min_connections: Option<u32>,
    /// The wait for a pooled connection, 5 s by default; see [`PgOptions::acquire_timeout_secs`].
    pub acquire_timeout_secs: Option<u64>,
}
