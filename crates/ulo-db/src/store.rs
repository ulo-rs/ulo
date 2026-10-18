use std::hash::Hash;

use serde::de::DeserializeOwned;
use ulo::{BoxError, BoxFuture, Secret};
use ulo_config::Problems;

/// What a driver crate supplies so [`StoreModule`](crate::StoreModule) can connect, check and close
/// its client. No macro is involved.
///
/// A store has no error type of its own, so nothing clashes with `ulo::ConnectError`. It marks an
/// error permanent, so no further attempt is made, by returning `Err(Permanent::new(e).into())`
/// from [`ping`](Self::ping), or from [`create`](Self::create) under
/// [`CREATE_CONNECTS`](Self::CREATE_CONNECTS).
pub trait Store: Send + Sync + 'static {
    /// `"postgres"`, `"mongodb"`: the check's name and the messages.
    const NAME: &'static str;

    /// The module's identity is by value, so equal options are one module.
    type Options: DeserializeOwned + Clone + Eq + Hash + Send + Sync;

    /// The library's own pool or client, bound bare so a consumer writes `Dep<sqlx::PgPool>`.
    type Client: Clone + Send + Sync + 'static;

    /// True when `create` cannot build the client without connecting. `ulo-db` then retries `create`
    /// in a loop of its own under `connect_within`, and the store offers no `.lazy()`. A `const`
    /// for the whole store.
    const CREATE_CONNECTS: bool;

    /// Checks the options at registration, which does no I/O. A failure is recorded through
    /// `try_value` with every other configuration problem, written from the failure's category and
    /// never from text that may echo the URL.
    fn validate(o: &Self::Options) -> Result<(), Problems>;

    /// The secrets `o` holds beyond the `Secret` leaves the section read already registered, such as
    /// a URL's password component, for `m.secret(..)`. A library error that prints the password
    /// alone is then scrubbed wherever the core stores it.
    fn secrets(o: &Self::Options) -> Vec<Secret<String>> {
        let _ = o;
        Vec::new()
    }

    /// Builds the client. Run through `spawn_with` on the app's runtime, so whatever the library
    /// spawns at creation runs there. Connects only under `CREATE_CONNECTS`.
    fn create(o: &Self::Options) -> BoxFuture<'static, Result<Self::Client, BoxError>>;

    /// One round trip: the `.ready` attempt and the health check.
    fn ping(c: &Self::Client) -> BoxFuture<'static, Result<(), BoxError>>;

    fn close(c: Self::Client) -> BoxFuture<'static, ()>;
}

/// A store whose `create` connects nothing, so [`StoreModule::lazy`](crate::StoreModule::lazy) is
/// offered: the start does not wait for the database, and the check stays down until the first ping
/// succeeds. Implemented exactly where `CREATE_CONNECTS` is `false`; elsewhere the method does not
/// exist, so the refusal is at compile time.
pub trait LazyStore: Store {}
