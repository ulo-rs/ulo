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

/// Checks the scheme, `mongodb://` or `mongodb+srv://`, and that a host follows it. Does no lookup:
/// registration does no I/O, and the SRV lookup runs in `create`.
pub(crate) fn validate(o: &MongoOptions) -> Result<(), Problems> {
    let _ = o;
    todo!()
}
