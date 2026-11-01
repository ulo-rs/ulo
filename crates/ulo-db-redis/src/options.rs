use serde::Deserialize;
use ulo::Secret;
use ulo_config::Problems;

/// Redis options. `Debug` prints the URL redacted.
///
/// The timeouts are in whole seconds and must not be zero. Unset, a connection attempt is bounded
/// at 5 s and a command at redis-rs's own default of 500 ms.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Deserialize)]
pub struct RedisOptions {
    /// `redis://` or `rediss://`. `rediss://` needs this crate's `tls` feature.
    pub url: Secret<String>,
    /// The bound on one connection attempt, which `ulo-db`'s loop repeats under `connect_within`.
    pub connection_timeout_secs: Option<u64>,
    /// The bound on one command once connected, the health ping included.
    pub response_timeout_secs: Option<u64>,
}

/// Checks the scheme and parses the URL through `redis::Client::open`, which does not echo it.
pub(crate) fn validate(o: &RedisOptions) -> Result<(), Problems> {
    let mut problems = Problems::new();
    check_url(o.url.expose(), &mut problems);
    if o.connection_timeout_secs == Some(0) {
        problems.at("connection_timeout_secs", "must be at least 1 second");
    }
    if o.response_timeout_secs == Some(0) {
        problems.at("response_timeout_secs", "must be at least 1 second");
    }
    problems.into_result()
}

fn check_url(url: &str, problems: &mut Problems) {
    // redis-rs also accepts `valkey://` and the unix-socket schemes; this store promises two, and
    // the check reads the scheme itself so that no message below can carry any part of the URL.
    let scheme = url.split_once("://").map(|(scheme, _)| scheme.to_ascii_lowercase());
    match scheme.as_deref() {
        Some("redis") => {}
        Some("rediss") if cfg!(feature = "tls") => {}
        Some("rediss") => {
            problems.at("url", "`rediss://` needs the `tls` feature of ulo-db-redis");
            return;
        }
        _ => {
            problems.at("url", "must start with `redis://` or `rediss://`");
            return;
        }
    }
    // The category is written here and the library's text is dropped, so a change in what redis-rs
    // prints cannot put the URL into a configuration report.
    if redis::Client::open(url).is_err() {
        problems.at("url", "does not parse as a Redis URL (host, port, database number or query)");
    }
}
