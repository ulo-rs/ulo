use std::sync::OnceLock;

use redis::aio::ConnectionManager;
use ulo::{BoxError, BoxFuture, Secret};
use ulo_config::Problems;
use ulo_db::Store;

use crate::options::RedisOptions;

/// The outcome of the process's one `prefer_tokio()` call, recorded for every later Redis module.
#[allow(dead_code)]
static TOKIO_PREFERENCE: OnceLock<Result<(), String>> = OnceLock::new();

/// Makes the process's one `prefer_tokio()` call, or reads its recorded outcome.
///
/// redis 1.7.1 offers `prefer_tokio()` only under `cfg(all(feature = "smol-comp", feature =
/// "tokio-comp"))` and no read of the preference, so a crate enabling `tokio-comp` alone cannot call
/// it. The call is built as the designer answers that premise.
#[allow(dead_code)]
pub(crate) fn prefer_tokio_once() -> Result<(), BoxError> {
    todo!()
}

/// The Redis store.
///
/// `AuthenticationFailed` and `InvalidClientConfig` are permanent; `Io` and a timeout are retryable.
pub struct Redis;

impl Store for Redis {
    const NAME: &'static str = "redis";
    type Options = RedisOptions;
    type Client = ConnectionManager;
    const CREATE_CONNECTS: bool = true;

    fn validate(o: &RedisOptions) -> Result<(), Problems> {
        crate::options::validate(o)
    }

    /// The URL's password component.
    fn secrets(o: &RedisOptions) -> Vec<Secret<String>> {
        let _ = o;
        todo!()
    }

    /// `ConnectionManager::new` with `set_number_of_retries(0)`: connects once. One attempt's bound
    /// inside `ulo-db`'s loop is the area's to decide.
    fn create(o: &RedisOptions) -> BoxFuture<'static, Result<ConnectionManager, BoxError>> {
        let _ = o;
        todo!()
    }

    fn ping(c: &ConnectionManager) -> BoxFuture<'static, Result<(), BoxError>> {
        let _ = c;
        todo!()
    }

    fn close(c: ConnectionManager) -> BoxFuture<'static, ()> {
        let _ = c;
        todo!()
    }
}
