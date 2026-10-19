use std::sync::OnceLock;
use std::time::Duration;

use redis::aio::{ConnectionManager, ConnectionManagerConfig};
use redis::{ErrorKind, RedisError};
use ulo::{BoxError, BoxFuture, Permanent, Secret};
use ulo_config::Problems;
use ulo_db::{LazyStore, Store};

use crate::options::RedisOptions;

/// One connection attempt's bound when the options set none. redis-rs's own default is 1 s, short
/// for a TLS handshake to a remote server; `ulo-db`'s loop repeats the attempt under
/// `connect_within`, so a longer bound here only caps the slowest single try.
const CONNECTION_TIMEOUT: Duration = Duration::from_secs(5);

/// The manager's own reconnect attempts per command, `set_number_of_retries`. It governs every
/// reconnect, not only the first connection (PROBES_db.md section 9): 0 makes a command fail at
/// once while the server is down, and a large value makes each command, and each `.ready` attempt,
/// wait out the whole backoff before it fails (9.5 s at 6, a wrong password included). At 3 an
/// attempt costs about 1.2 s and a command rides out an outage of about that length.
const RECONNECT_RETRIES: usize = 3;

/// The outcome of the process's one runtime-preference step, recorded for every later Redis module.
static TOKIO_PREFERENCE: OnceLock<Result<(), String>> = OnceLock::new();

/// Makes the process's one runtime-preference step, or reads its recorded outcome.
///
/// redis 1.7.1 defines `prefer_tokio()` only when both `smol-comp` and `tokio-comp` are compiled
/// in, and offers no read of the current preference, so a crate that enables `tokio-comp` alone
/// cannot call it. The step is therefore the guard without the call: the outcome is recorded once
/// and is `Ok`. It does not make redis-rs prefer tokio; it does not need to unless an unrelated
/// crate compiled `smol-comp` in and set `prefer_smol()`, a conflict this crate cannot see. The
/// caller runs on the app's runtime after `ulo-db`'s name check, so redis-rs, absent a preference,
/// finds tokio's context and uses tokio.
pub(crate) fn prefer_tokio_once() -> Result<(), BoxError> {
    let outcome = TOKIO_PREFERENCE.get_or_init(|| {
        tracing::debug!(
            target: "ulo_db_redis",
            "redis-rs's runtime preference is not set: prefer_tokio() exists only with smol-comp compiled in"
        );
        Ok(())
    });
    match outcome {
        Ok(()) => Ok(()),
        Err(conflict) => Err(Permanent::new(conflict.clone()).into()),
    }
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

impl LazyStore for Redis {}

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
    /// unless the options set it, and repeats up to [`RECONNECT_RETRIES`] times inside the command
    /// that made it.
    fn create(o: &RedisOptions) -> BoxFuture<'static, Result<ConnectionManager, BoxError>> {
        let url = o.url.clone();
        let connection_timeout = o.connection_timeout_secs.map_or(CONNECTION_TIMEOUT, Duration::from_secs);
        let response_timeout = o.response_timeout_secs.map(Duration::from_secs);
        Box::pin(async move {
            prefer_tokio_once()?;
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
