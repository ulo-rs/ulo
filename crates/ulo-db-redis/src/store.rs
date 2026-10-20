use std::sync::OnceLock;
use std::time::Duration;

use redis::aio::{ConnectionManager, ConnectionManagerConfig};
use redis::{ErrorKind, RedisError};
use ulo::{BoxError, BoxFuture, Permanent, Secret};
use ulo_config::Problems;
use ulo_db::Store;

use crate::options::RedisOptions;

/// One connection attempt's bound when the options set none. redis-rs's own default is 1 s, short
/// for a TLS handshake to a remote server. With [`RECONNECT_RETRIES`] at 0 one command that connects
/// makes one try, so against a host that never answers one `.ready` attempt lasts this bound;
/// `.ready` repeats the attempt under `connect_within`.
const CONNECTION_TIMEOUT: Duration = Duration::from_secs(5);

/// The manager's own reconnect attempts per command, `set_number_of_retries`, switched off so that
/// `.ready` is the one retry policy (DESIGN decision 48). It governs every reconnect, not only the
/// first connection (PROBES_db.md sections 9 and 9.1): a wrong password fails in 2 ms instead of
/// 1.2 s at 3 and 9.5 s at 6. Recovery from an outage takes the same time at every setting, so the
/// retries buy a command nothing but a longer wait before it fails.
const RECONNECT_RETRIES: usize = 0;

/// Set once the process's runtime-preference step has run.
static TOKIO_PREFERENCE: OnceLock<()> = OnceLock::new();

/// Makes the process's one runtime-preference step.
///
/// redis 1.7.1 defines `prefer_tokio()` only when both `smol-comp` and `tokio-comp` are compiled
/// in, so the call exists here only under this crate's own `smol-comp` feature. With the feature
/// on, a refusal because another crate set a preference first is logged at `warn` and the start
/// continues: redis 1.7.1 offers no read of the preference, so a harmless double setting cannot be
/// told from a harmful one. With the feature off there is nothing to call and nothing is logged;
/// redis-rs, absent a preference, finds tokio's context on the app's runtime and uses tokio.
pub(crate) fn prefer_tokio_once() {
    TOKIO_PREFERENCE.get_or_init(|| {
        #[cfg(feature = "smol-comp")]
        if let Err(refused) = redis::aio::prefer_tokio() {
            tracing::warn!(
                target: "ulo_db_redis",
                error = %refused,
                "another crate set redis-rs's runtime preference first; if it is not tokio, the first command fails"
            );
        }
    });
}

/// An error that no further attempt can mend is wrapped in `Permanent`; the rest are bare.
fn classify(error: RedisError) -> BoxError {
    if matches!(error.kind(), ErrorKind::AuthenticationFailed | ErrorKind::InvalidClientConfig) {
        Permanent::new(error).into()
    } else {
        error.into()
    }
}

/// The Redis store.
///
/// `create` builds the manager without connecting; the first command, which is the first ping under
/// `.ready` or under `.lazy()`, makes the connection. `.ready` is therefore the retry loop.
/// `AuthenticationFailed` and `InvalidClientConfig` are permanent, so a wrong password ends the
/// start at its first attempt; `Io` and a timeout are retryable.
pub struct Redis;

impl Store for Redis {
    const NAME: &'static str = "redis";
    type Options = RedisOptions;
    type Client = ConnectionManager;

    fn create_connects(_: &RedisOptions) -> bool {
        false
    }

    fn validate(o: &RedisOptions) -> Result<(), Problems> {
        crate::options::validate(o)
    }

    /// The URL's password component.
    fn secrets(o: &RedisOptions) -> Vec<Secret<String>> {
        let Ok(client) = redis::Client::open(o.url.expose().as_str()) else {
            return Vec::new();
        };
        match client.get_connection_info().redis_settings().password() {
            Some(password) if !password.is_empty() => vec![Secret::new(password.to_owned())],
            _ => Vec::new(),
        }
    }

    /// `ConnectionManager::new_lazy_with_config`: synchronous, connects nothing and fails only on a
    /// URL the client cannot open. A connection attempt is bounded by the connection timeout, 5 s
    /// unless the options set it, and is not repeated inside the command that made it.
    fn create(o: &RedisOptions) -> BoxFuture<'static, Result<ConnectionManager, BoxError>> {
        let url = o.url.clone();
        let connection_timeout = o.connection_timeout_secs.map_or(CONNECTION_TIMEOUT, Duration::from_secs);
        let response_timeout = o.response_timeout_secs.map(Duration::from_secs);
        Box::pin(async move {
            prefer_tokio_once();
            let client = redis::Client::open(url.expose().as_str()).map_err(classify)?;
            let mut config = ConnectionManagerConfig::new()
                .set_number_of_retries(RECONNECT_RETRIES)
                .set_connection_timeout(Some(connection_timeout));
            if response_timeout.is_some() {
                config = config.set_response_timeout(response_timeout);
            }
            ConnectionManager::new_lazy_with_config(client, config).map_err(classify)
        })
    }

    /// `PING`. The first one connects, so it also reports an unreachable server, a refused password
    /// (`AuthenticationFailed`, permanent) and a TLS failure.
    fn ping(c: &ConnectionManager) -> BoxFuture<'static, Result<(), BoxError>> {
        let mut connection = c.clone();
        Box::pin(async move {
            redis::cmd("PING").query_async::<String>(&mut connection).await.map(|_| ()).map_err(classify)
        })
    }

    /// `ConnectionManager` has no close: its task ends when the last clone drops, so a consumer
    /// still holding a clone keeps the connection open past this call.
    fn close(c: ConnectionManager) -> BoxFuture<'static, ()> {
        Box::pin(async move { drop(c) })
    }
}

#[cfg(all(test, feature = "smol-comp"))]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use tracing::span::{Attributes, Id, Record};
    use tracing::{Event, Level, Metadata, Subscriber};

    use super::prefer_tokio_once;

    struct Warnings(Arc<AtomicUsize>);

    impl Subscriber for Warnings {
        fn enabled(&self, _: &Metadata<'_>) -> bool {
            true
        }
        fn new_span(&self, _: &Attributes<'_>) -> Id {
            Id::from_u64(1)
        }
        fn record(&self, _: &Id, _: &Record<'_>) {}
        fn record_follows_from(&self, _: &Id, _: &Id) {}
        fn event(&self, event: &Event<'_>) {
            if *event.metadata().level() == Level::WARN {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
        fn enter(&self, _: &Id) {}
        fn exit(&self, _: &Id) {}
    }

    /// Another crate's preference set first: the refusal is a `warn`, once, and nothing else.
    #[test]
    fn a_refused_preference_is_a_warning() {
        redis::aio::prefer_smol().expect("no preference was set yet in this process");
        let seen = Arc::new(AtomicUsize::new(0));
        tracing::subscriber::with_default(Warnings(Arc::clone(&seen)), || {
            prefer_tokio_once();
            prefer_tokio_once();
        });
        assert_eq!(seen.load(Ordering::SeqCst), 1, "one warning per process for a refused preference");
    }
}
