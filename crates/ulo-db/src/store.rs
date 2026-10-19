use std::hash::Hash;

use serde::de::DeserializeOwned;
use ulo::{BoxError, BoxFuture, Secret};
use ulo_config::Problems;

/// What a driver crate supplies so [`StoreModule`](crate::StoreModule) can connect, check and close
/// its client. No macro is involved.
///
/// A store has no error type of its own, so nothing clashes with `ulo::ConnectError`. It marks an
/// error permanent, so no further attempt is made, by returning `Err(Permanent::new(e).into())`
/// from [`ping`](Self::ping), or from [`create`](Self::create) when
/// [`create_connects`](Self::create_connects) answers true.
pub trait Store: Send + Sync + 'static {
    /// `"postgres"`, `"mongodb"`: the check's name and the messages.
    const NAME: &'static str;

    /// The module's identity is by value, so equal options are one module.
    type Options: DeserializeOwned + Clone + Eq + Hash + Send + Sync;

    /// The library's own pool or client, bound bare so a consumer writes `Dep<sqlx::PgPool>`.
    type Client: Clone + Send + Sync + 'static;

    /// True when `create` cannot build the client for `o` without connecting or resolving a name
    /// over the network. `ulo-db` then retries `create` in a loop of its own under `connect_within`,
    /// and `.lazy()` on that module is refused at registration. A store answers from its nature,
    /// the same for every `o`, unless the options decide it: MongoDB's `mongodb+srv://` URI
    /// resolves DNS in `create` and a plain `mongodb://` URI does not.
    fn create_connects(o: &Self::Options) -> bool;

    /// What the registration refusal of `.lazy()` says about why `create` connects for `o`, naming
    /// what in the options decides it. Read only when [`create_connects`](Self::create_connects)
    /// answers true.
    fn connects_because(o: &Self::Options) -> &'static str {
        let _ = o;
        "its `create` connects"
    }

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
    /// spawns at creation runs there. Connects only where `create_connects` answers true.
    fn create(o: &Self::Options) -> BoxFuture<'static, Result<Self::Client, BoxError>>;

    /// One round trip: the `.ready` attempt and the health check.
    fn ping(c: &Self::Client) -> BoxFuture<'static, Result<(), BoxError>>;

    /// Closes the client in its binding's `on_destroy`, after every transport has drained, through
    /// `spawn_with` under the hook's bound. An execution the drain abandoned may still hold a
    /// connection, so a close that waits for checked-out connections is cut by that bound.
    fn close(c: Self::Client) -> BoxFuture<'static, ()>;
}

/// A store that may be lazy, so [`StoreModule::lazy`](crate::StoreModule::lazy) is offered: the
/// start does not wait for the database, and the check stays down until the first ping succeeds.
/// Whether the options allow it is [`Store::create_connects`], which is read at registration: a
/// module over options for which it answers true refuses `.lazy()` there, through `try_value`.
/// A store whose `create` connects for every option leaves this marker unimplemented, and the
/// method does not exist for it.
pub trait LazyStore: Store {}
