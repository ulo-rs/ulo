use mongodb::bson::doc;
use mongodb::error::{Error as MongoError, ErrorKind};
use mongodb::options::ClientOptions;
use mongodb::Client;
use ulo::{BoxError, BoxFuture, Permanent, Secret};
use ulo_config::Problems;
use ulo_db::{LazyStore, Store};

use crate::options::MongoOptions;

/// The MongoDB store.
///
/// `create` parses the URI, which resolves a `mongodb+srv` host's DNS, and builds the client, which
/// connects nothing. A `mongodb+srv://` URI therefore answers
/// [`create_connects`](Store::create_connects) true: `ulo-db` retries `create` under
/// `connect_within`, and `.lazy()` on that module is refused at registration. A plain `mongodb://`
/// URI resolves nothing, so `create` returns at once, `.lazy()` is offered, and `.ready(ping)` is
/// the retry loop.
///
/// Permanent, so no further attempt: `ErrorKind::Authentication`, `InvalidArgument`,
/// `InvalidTlsConfig`, `IncompatibleServer` and a command refused as `AuthenticationFailed`.
/// Retryable: `ServerSelection`, and a failed SRV or TXT lookup (`DnsResolve`), which cannot be told
/// apart from a resolver that is briefly unreachable and is bounded by `connect_within` either way.
pub struct Mongo;

fn is_permanent(e: &MongoError) -> bool {
    match &*e.kind {
        ErrorKind::Authentication { .. }
        | ErrorKind::InvalidArgument { .. }
        | ErrorKind::InvalidTlsConfig { .. }
        | ErrorKind::IncompatibleServer { .. } => true,
        ErrorKind::Command(c) => c.code == AUTHENTICATION_FAILED,
        _ => false,
    }
}

/// The server's `AuthenticationFailed` code, which a command can answer when the handshake did not.
const AUTHENTICATION_FAILED: i32 = 18;

fn classify(e: MongoError) -> BoxError {
    if is_permanent(&e) { Permanent::new(e).into() } else { e.into() }
}

/// Decodes `%XX` escapes. A malformed escape is kept as written, since the result only widens the
/// set of texts the redaction looks for.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let escaped = (bytes[i] == b'%')
            .then(|| bytes.get(i + 1..i + 3))
            .flatten()
            .and_then(|h| std::str::from_utf8(h).ok())
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match escaped {
            Some(b) => {
                out.push(b);
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

impl LazyStore for Mongo {}

impl Store for Mongo {
    const NAME: &'static str = "mongodb";
    type Options = MongoOptions;
    type Client = Client;

    fn create_connects(o: &MongoOptions) -> bool {
        o.uri.expose().starts_with(crate::options::SRV)
    }

    fn connects_because(_: &MongoOptions) -> &'static str {
        "a `mongodb+srv://` URI resolves its DNS inside `create`, which the retry loop must carry; use a `mongodb://` URI to be lazy"
    }

    fn validate(o: &MongoOptions) -> Result<(), Problems> {
        crate::options::validate(o)
    }

    /// The URI's password component, as written and percent-decoded: a driver error that prints the
    /// password prints the decoded form.
    fn secrets(o: &MongoOptions) -> Vec<Secret<String>> {
        let Some(raw) = crate::options::password(o.uri.expose()) else {
            return Vec::new();
        };
        let decoded = percent_decode(raw);
        let mut out = vec![Secret::new(raw.to_owned())];
        if decoded != raw && !decoded.is_empty() {
            out.push(Secret::new(decoded));
        }
        out
    }

    /// `ClientOptions::parse`, with the SRV lookup, then `Client::with_options`. `ulo-db` runs the
    /// future on the app's runtime through `spawn_with`, where the monitor tasks the client spawns
    /// find tokio's context. A field the options set overrides the same setting in the URI.
    fn create(o: &MongoOptions) -> BoxFuture<'static, Result<Client, BoxError>> {
        let o = o.clone();
        Box::pin(async move {
            let mut client_options =
                ClientOptions::parse(o.uri.expose().as_str()).await.map_err(classify)?;
            if o.max_pool_size.is_some() {
                client_options.max_pool_size = o.max_pool_size;
            }
            if o.min_pool_size.is_some() {
                client_options.min_pool_size = o.min_pool_size;
            }
            if o.app_name.is_some() {
                client_options.app_name = o.app_name.clone();
            }
            Client::with_options(client_options).map_err(classify)
        })
    }

    fn ping(c: &Client) -> BoxFuture<'static, Result<(), BoxError>> {
        let c = c.clone();
        Box::pin(async move {
            c.database("admin").run_command(doc! { "ping": 1 }).await.map(drop).map_err(classify)
        })
    }

    /// A graceful shutdown waits for live cursors and sessions; the destroy hook's bound ends the
    /// wait, so a leaked cursor costs that bound and nothing else.
    fn close(c: Client) -> BoxFuture<'static, ()> {
        Box::pin(async move { c.shutdown().await })
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use ulo::{App, StartupError, WiringError};
    use ulo_db::StoreModule;

    use super::Mongo;
    use crate::MongoOptions;

    fn module(uri: &str) -> StoreModule<Mongo> {
        let options: MongoOptions = serde_json::from_value(json!({ "uri": uri })).unwrap();
        StoreModule::from_options(options)
    }

    /// The registration refusals the module records through `try_value`. The app has no runtime,
    /// so `wire()` also reports the missing `Dep<dyn Runtime>`, which is not what these tests read.
    fn refusals(module: StoreModule<Mongo>) -> Vec<String> {
        match App::builder(module).wire() {
            Ok(_) => Vec::new(),
            Err(StartupError::Wiring(errors)) => errors
                .iter()
                .filter_map(|e| match e {
                    WiringError::ValueFailed { error, .. } => Some(error.to_string()),
                    _ => None,
                })
                .collect(),
            Err(other) => panic!("wiring failed outside the wiring report: {other}"),
        }
    }

    #[test]
    fn lazy_on_a_srv_uri_is_refused_at_registration_naming_the_scheme() {
        let refused = refusals(module("mongodb+srv://cluster.example.net/").lazy());
        assert_eq!(refused.len(), 1, "{refused:?}");
        assert!(refused[0].contains("mongodb+srv://"), "the refusal does not name the scheme: {refused:?}");
        assert!(refused[0].contains("cannot be lazy"), "{refused:?}");
    }

    #[test]
    fn lazy_on_a_plain_uri_is_not_refused() {
        assert_eq!(refusals(module("mongodb://127.0.0.1:27017/").lazy()), Vec::<String>::new());
    }

    #[test]
    fn a_srv_uri_without_lazy_is_not_refused() {
        assert_eq!(refusals(module("mongodb+srv://cluster.example.net/")), Vec::<String>::new());
    }
}
