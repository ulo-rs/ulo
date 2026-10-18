use serde::Deserialize;
use ulo::Secret;
use ulo_config::Problems;
use ulo_db_sqlx::{Engine, validate_url};

/// SeaORM options. `Debug` prints the URL redacted, and no `ConnectOptions` value is formatted.
///
/// A pool of `max_connections` at 1 cannot run migrations on Postgres or MySQL: the migration lock
/// holds one pooled connection while the migrator takes another.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Deserialize)]
pub struct SeaOrmOptions {
    /// A `postgres://`, `mysql://` or `sqlite:` URL.
    pub url: Secret<String>,
    pub max_connections: Option<u32>,
    pub min_connections: Option<u32>,
    pub acquire_timeout_secs: Option<u64>,
}

/// The scheme check `ulo-db-sqlx` shares, then SQLx's option parser for the engine the scheme names.
///
/// The engine comes from the schemes `Database::connect` routes on (`postgres`, `postgresql`,
/// `mysql`, `sqlite`), not from `Engine::schemes`, which may name more: a scheme SQLx accepts and
/// SeaORM does not would pass here and fail in `Database::connect` with the URL in its message.
pub(crate) fn validate(o: &SeaOrmOptions) -> Result<(), Problems> {
    let url = o.url.expose();
    let mut problems = Problems::new();

    match engine_of(url) {
        None => problems.at("url", format!("the scheme must be {}", expected_schemes())),
        Some(engine) if !enabled(engine) => problems.at(
            "url",
            format!("the engine this scheme names is not compiled into ulo-db-seaorm; the scheme must be {}", expected_schemes()),
        ),
        Some(engine) => {
            if let Err(found) = validate_url(engine, url) {
                for problem in found.iter() {
                    problems.at(problem.path(), problem.message());
                }
            }
        }
    }

    if o.max_connections == Some(0) {
        problems.at("max_connections", "must be at least 1");
    }
    if let (Some(min), Some(max)) = (o.min_connections, o.max_connections)
        && min > max
    {
        problems.at("min_connections", "must not exceed max_connections");
    }
    if o.acquire_timeout_secs == Some(0) {
        problems.at("acquire_timeout_secs", "must be at least 1");
    }

    problems.into_result()
}

fn engine_of(url: &str) -> Option<Engine> {
    let (scheme, _) = url.split_once(':')?;
    match scheme.to_ascii_lowercase().as_str() {
        "postgres" | "postgresql" => Some(Engine::Postgres),
        "mysql" => Some(Engine::MySql),
        "sqlite" => Some(Engine::Sqlite),
        _ => None,
    }
}

// With every engine feature on the arms are all `true`, which clippy reads as `matches!`.
#[allow(clippy::match_like_matches_macro)]
fn enabled(engine: Engine) -> bool {
    match engine {
        Engine::Postgres => cfg!(feature = "postgres"),
        Engine::MySql => cfg!(feature = "mysql"),
        Engine::Sqlite => cfg!(feature = "sqlite"),
        _ => false,
    }
}

fn expected_schemes() -> String {
    let mut names: Vec<&str> = Vec::new();
    if cfg!(feature = "postgres") {
        names.push("postgres:// or postgresql://");
    }
    if cfg!(feature = "mysql") {
        names.push("mysql://");
    }
    if cfg!(feature = "sqlite") {
        names.push("sqlite:");
    }
    if names.is_empty() {
        return "none: enable the `postgres`, `mysql` or `sqlite` feature of ulo-db-seaorm".to_owned();
    }
    names.join(", ")
}

/// Every text under which the URL's password can appear in a library's message: as written, and
/// percent-decoded. The `password` query parameter, which SQLx's Postgres parser reads, is
/// decoded the way a query is, with `+` for a space.
pub(crate) fn password_texts(url: &str) -> Vec<String> {
    let mut found = Vec::new();
    let Some((_, rest)) = url.split_once("://") else {
        return found;
    };
    let rest = rest.split('#').next().unwrap_or(rest);
    let (before_query, query) = match rest.split_once('?') {
        Some((before, query)) => (before, Some(query)),
        None => (rest, None),
    };

    let authority = before_query.split('/').next().unwrap_or("");
    if let Some((userinfo, _)) = authority.rsplit_once('@')
        && let Some((_, password)) = userinfo.split_once(':')
    {
        add(&mut found, password.to_owned());
        add(&mut found, percent_decode(password));
    }

    for pair in query.into_iter().flat_map(|q| q.split('&')) {
        if let Some(("password", value)) = pair.split_once('=') {
            add(&mut found, value.to_owned());
            add(&mut found, percent_decode(value));
            add(&mut found, percent_decode(&value.replace('+', " ")));
        }
    }
    found
}

/// `text` with the URL and every password text replaced by the redaction marker, URL first since
/// it contains the password.
pub(crate) fn scrub(text: &str, url: &str) -> String {
    let mut out = text.to_owned();
    let mut secrets = vec![url.to_owned()];
    secrets.extend(password_texts(url));
    for secret in secrets {
        if !secret.is_empty() {
            out = out.replace(&secret, "[redacted]");
        }
    }
    out
}

fn add(found: &mut Vec<String>, text: String) {
    if !text.is_empty() && !found.contains(&text) {
        found.push(text);
    }
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let high = (bytes[i + 1] as char).to_digit(16);
            let low = (bytes[i + 2] as char).to_digit(16);
            if let (Some(high), Some(low)) = (high, low) {
                out.push((high * 16 + low) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}
