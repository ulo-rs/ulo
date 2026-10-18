use serde::Deserialize;
use ulo::Secret;
use ulo_config::Problems;

/// MongoDB options. `Debug` prints the URI redacted.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Deserialize)]
pub struct MongoOptions {
    /// `mongodb://` or `mongodb+srv://`, MongoDB's connection-string specification.
    pub uri: Secret<String>,
    pub max_pool_size: Option<u32>,
    pub min_pool_size: Option<u32>,
    pub app_name: Option<String>,
}

const PLAIN: &str = "mongodb://";
const SRV: &str = "mongodb+srv://";

/// The part of `uri` after the scheme up to the first `/` or `?`: userinfo, hosts and ports. Answers
/// the scheme's SRV-ness beside it, or `None` for a scheme the store does not take.
fn authority(uri: &str) -> Option<(bool, &str)> {
    let (srv, rest) = match uri.strip_prefix(SRV) {
        Some(rest) => (true, rest),
        None => (false, uri.strip_prefix(PLAIN)?),
    };
    let end = rest.find(['/', '?']).unwrap_or(rest.len());
    Some((srv, &rest[..end]))
}

/// The hosts of an authority, userinfo removed. The split is at the last `@`, as the connection
/// string specification has it: the userinfo's own `@` must be percent-encoded, so the last one
/// ends it.
fn hosts(authority: &str) -> &str {
    authority.rsplit_once('@').map_or(authority, |(_, hosts)| hosts)
}

/// The password component of the URI's userinfo as written, still percent-encoded. `None` when
/// there is no userinfo, no `:` in it, or the password is empty.
pub(crate) fn password(uri: &str) -> Option<&str> {
    let (_, authority) = authority(uri)?;
    let (userinfo, _) = authority.rsplit_once('@')?;
    let (_, password) = userinfo.split_once(':')?;
    (!password.is_empty()).then_some(password)
}

/// Checks the scheme, `mongodb://` or `mongodb+srv://`, and that a host follows it. Does no lookup:
/// registration does no I/O, and the SRV lookup runs in `create`.
///
/// Problems are written from the failure's category. None carries any part of the URI, whose
/// userinfo holds the password.
pub(crate) fn validate(o: &MongoOptions) -> Result<(), Problems> {
    let mut problems = Problems::new();
    match authority(o.uri.expose()) {
        None => problems.at("uri", "the scheme must be `mongodb://` or `mongodb+srv://`"),
        Some((srv, authority)) => {
            let hosts = hosts(authority);
            if hosts.split(',').any(str::is_empty) {
                problems.at("uri", "the URI names no host, or an empty one in its host list");
            } else if srv && hosts.contains(',') {
                // The driver refuses this in `parse` as `InvalidArgument`; refusing it at
                // registration puts the failure on the `uri` path before any connect.
                problems.at("uri", "a `mongodb+srv://` URI takes exactly one host");
            }
        }
    }
    problems.into_result()
}
