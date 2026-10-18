use serde::Deserialize;
use ulo::Secret;
#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
use ulo_config::Problems;

/// Options for every engine: the URL, which `Debug` prints redacted, and the pool's size and wait.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Deserialize)]
pub struct DieselOptions {
    /// `postgres://` or `postgresql://`, `mysql://`, or for SQLite a path, `:memory:`, a `file:`
    /// URI or `sqlite://<path>`.
    pub url: Secret<String>,
    /// The pool's largest size, deadpool's default when unset. Zero is refused.
    pub max_size: Option<usize>,
    /// How long a caller waits for a connection while the pool is at its size. Unset, the caller
    /// waits until one is returned.
    pub wait_timeout_secs: Option<u64>,
}

/// Checks the URL's scheme against `schemes`, each written without its `://`; the problem names the
/// schemes expected and prints no part of the URL.
#[cfg(any(feature = "postgres", feature = "mysql"))]
pub(crate) fn check_scheme(o: &DieselOptions, schemes: &[&str]) -> Result<(), Problems> {
    // The drivers compare the scheme case-sensitively, so this does too.
    let accepted = o.url.expose().split_once("://").is_some_and(|(scheme, _)| schemes.contains(&scheme));
    if accepted {
        return Ok(());
    }
    let expected: Vec<String> = schemes.iter().map(|s| format!("`{s}://`")).collect();
    let mut problems = Problems::new();
    problems.at("url", format!("expected a URL starting with {}", expected.join(" or ")));
    Err(problems)
}

/// The scheme, then tokio-postgres's own parser, the one `AsyncPgConnection::establish` runs, then
/// a host or `hostaddr`, which that parser leaves to `connect` to require.
#[cfg(feature = "postgres")]
pub(crate) fn validate_postgres(o: &DieselOptions) -> Result<(), Problems> {
    let mut problems = check_scheme(o, &["postgres", "postgresql"]).err().unwrap_or_default();
    if problems.is_empty() {
        match o.url.expose().parse::<tokio_postgres::Config>() {
            Ok(config) if config.get_hosts().is_empty() && config.get_hostaddrs().is_empty() => {
                problems.at("url", "names no host")
            }
            Ok(_) => {}
            Err(e) => problems.at("url", describe("postgres", o.url.expose(), &e.to_string())),
        }
    }
    check_pool(o, &mut problems);
    problems.into_result()
}

/// The scheme, then mysql_async's own parser, the one `AsyncMysqlConnection::establish` runs.
#[cfg(feature = "mysql")]
pub(crate) fn validate_mysql(o: &DieselOptions) -> Result<(), Problems> {
    let mut problems = check_scheme(o, &["mysql"]).err().unwrap_or_default();
    if problems.is_empty()
        && let Err(e) = mysql_async::Opts::from_url(o.url.expose())
    {
        problems.at("url", describe("mysql", o.url.expose(), &e.to_string()));
    }
    check_pool(o, &mut problems);
    problems.into_result()
}

/// SQLite has no URL parser to call: `SqliteConnection::establish` opens whatever text it is given,
/// after rewriting a leading `sqlite://` to `file:`. The check refuses the spellings that would open
/// something other than what was meant.
#[cfg(feature = "sqlite")]
pub(crate) fn validate_sqlite(o: &DieselOptions) -> Result<(), Problems> {
    let url = o.url.expose();
    let mut problems = Problems::new();
    let expected = "a path, `:memory:`, a `file:` URI or `sqlite://<path>`";
    if url.is_empty() {
        problems.at("url", format!("is empty; expected {expected}"));
    } else if url.contains('\0') {
        problems.at("url", "contains a NUL byte");
    } else if let Some((scheme, _)) = url.split_once("://")
        && scheme != "sqlite"
        && scheme != "file"
    {
        problems.at("url", format!("expected {expected}"));
    } else if url.starts_with("sqlite:") && !url.starts_with("sqlite://") {
        problems.at(
            "url",
            "`sqlite:<path>` opens a file named after the whole text under diesel; write `sqlite://<path>` or the path",
        );
    }
    check_pool(o, &mut problems);
    problems.into_result()
}

#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
fn check_pool(o: &DieselOptions, problems: &mut Problems) {
    if o.max_size == Some(0) {
        problems.at("max_size", "must be at least 1");
    }
}

/// The problem for a URL its driver refused. The driver's text names a category such as an invalid
/// port and is kept only while it holds none of the URL's sensitive pieces, in case a parser quotes
/// its input.
#[cfg(any(feature = "postgres", feature = "mysql"))]
fn describe(name: &str, url: &str, detail: &str) -> String {
    if leaks(url, detail) {
        format!("is not a valid {name} connection URL")
    } else {
        format!("is not a valid {name} connection URL: {detail}")
    }
}

#[cfg(any(feature = "postgres", feature = "mysql"))]
fn leaks(url: &str, text: &str) -> bool {
    let mut pieces = password_forms(url);
    if let Some(userinfo) = userinfo(url) {
        pieces.push(userinfo.to_owned());
    }
    pieces.push(url.to_owned());
    pieces.iter().any(|p| !p.is_empty() && text.contains(p.as_str()))
}

/// The password component of `url` in each spelling a driver error might print, for
/// `Store::secrets`. The whole URL is registered by the section read, as every `Secret` leaf is.
#[cfg(any(feature = "postgres", feature = "mysql"))]
pub(crate) fn password_secrets(url: &str) -> Vec<Secret<String>> {
    password_forms(url).into_iter().map(Secret::new).collect()
}

/// Everything before the last `@` of a `scheme://userinfo@host` URL's authority.
#[cfg(any(feature = "postgres", feature = "mysql"))]
fn userinfo(url: &str) -> Option<&str> {
    let (_, rest) = url.split_once("://")?;
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (userinfo, _) = rest[..end].rsplit_once('@')?;
    Some(userinfo)
}

/// The password as written and percent-decoded, from the userinfo and from a `password=` query
/// parameter, which libpq's URI form allows. A driver error prints the decoded form; the URL holds
/// the written one.
#[cfg(any(feature = "postgres", feature = "mysql"))]
fn password_forms(url: &str) -> Vec<String> {
    let mut forms = Vec::new();
    if let Some((_, password)) = userinfo(url).and_then(|u| u.split_once(':')) {
        forms.push(password.to_owned());
        forms.push(percent_decode(password));
    }
    let query = url.split_once('?').map_or("", |(_, q)| q.split('#').next().unwrap_or(q));
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

#[cfg(any(feature = "postgres", feature = "mysql"))]
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let escaped = match (bytes[i], bytes.get(i + 1), bytes.get(i + 2)) {
            (b'%', Some(&high), Some(&low)) => char::from(high).to_digit(16).zip(char::from(low).to_digit(16)),
            _ => None,
        };
        match escaped {
            Some((high, low)) => {
                out.push((high * 16 + low) as u8);
                i += 3;
            }
            None => {
                out.push(bytes[i]);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}
