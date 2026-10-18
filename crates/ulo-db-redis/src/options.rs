use serde::Deserialize;
use ulo::Secret;
use ulo_config::Problems;

/// Redis options. `Debug` prints the URL redacted.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Deserialize)]
pub struct RedisOptions {
    /// `redis://` or `rediss://`.
    pub url: Secret<String>,
    pub connection_timeout_secs: Option<u64>,
    pub response_timeout_secs: Option<u64>,
}

/// Checks the scheme and parses the URL through `redis::Client::open`, which does not echo it.
pub(crate) fn validate(o: &RedisOptions) -> Result<(), Problems> {
    let _ = o;
    todo!()
}
