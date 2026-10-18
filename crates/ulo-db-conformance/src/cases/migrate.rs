use futures_util::future::join;
use ulo::{App, Connected, StartupError};
use ulo_db::{MigrationFailed, Store};

use crate::cases::{Root, builder, carried, closed, connected, store, wired};
use crate::{Host, MigrationProbe, report};

/// Fails the scenario when the host declares no migrator for its store.
fn require_migrations<H: Host>(host: &H) {
    assert!(
        host.capabilities().migrations,
        "the host declares `{}` without `.migrate(..)`: declare this scenario not applicable",
        <H::Store as Store>::NAME
    );
}

/// Migrations run only on request. Planted break: migrations run without `.migrate(..)`.
///
/// An app whose module has no `.migrate(..)` connects and closes first. An app whose module is
/// given the host's migrations through [`Host::migrating`] then wires without running them, since
/// `wire` builds nothing, and has run them once when `connect` returns, in the module's init hook:
/// had the first app applied them, nothing would be pending and the count would stay at zero. A
/// second app given the same migrations finds nothing pending, and the count stays at one. A
/// migration run at registration, or run again over a schema that holds it, moves the count off the
/// one asserted.
pub async fn only_on_request<H: Host>() {
    let host = H::start().await;
    require_migrations(&host);
    let probe = MigrationProbe::new();

    let plain = connected(builder::<H>(Root::new().import(store::<H>(host.options()))), "an app without `.migrate(..)`").await;
    closed(plain, "the app without `.migrate(..)`").await;

    let app = wired(
        builder::<H>(Root::new().import(host.migrating(store::<H>(host.options()), probe.clone()))),
        "an app with `.migrate(..)`",
    );
    assert_eq!(probe.runs(), 0, "migrations ran at `wire`, which builds nothing");
    let app = match app.connect().await {
        Ok(app) => app,
        Err(error) => panic!("an app with `.migrate(..)` did not connect{}: {}", migration_failure(&error), report(&error)),
    };
    assert_eq!(
        probe.runs(),
        1,
        "the requested migrations did not run once by the end of `connect`: none pending means an app that did not ask for them applied them"
    );
    closed(app, "the app with `.migrate(..)`").await;

    let again = connected(
        builder::<H>(Root::new().import(host.migrating(store::<H>(host.options()), probe.clone()))),
        "a second app with `.migrate(..)`",
    )
    .await;
    assert_eq!(probe.runs(), 1, "migrations already applied ran again in a second app");
    closed(again, "the second app with `.migrate(..)`").await;
}

/// Two replicas started against one server apply the migrations once, the second waiting on the
/// lock. The scenario uses ordinary DDL, since `CREATE INDEX CONCURRENTLY` can deadlock against a
/// session waiting on an advisory lock. Planted break: the migration lock removed.
///
/// The replicas are two apps in one process, each with its own graph and its own client, built
/// from the host's options with the same migrations, so the server sees two clients as it would
/// two processes. Both are wired first and then connected together, so their connects overlap from
/// the first: the creation, the readiness ping, and the migrations in each init hook. Both connect,
/// the second having waited on the lock and found nothing pending, and the migrations ran once.
/// Without the lock, a migrator that takes none (SeaORM's, `diesel_migrations`) runs the body in
/// both, and the loser fails its start on the version table or the count reaches two; SQLx's
/// migrator takes a lock of its own and may still pass.
pub async fn once_across_replicas<H: Host>() {
    let host = H::start().await;
    require_migrations(&host);
    let probe = MigrationProbe::new();
    let replica = |n: &str| {
        wired(
            builder::<H>(Root::new().import(host.migrating(store::<H>(host.options()), probe.clone()))),
            &format!("replica {n}"),
        )
    };
    let (first, second) = (replica("1"), replica("2"));
    let (first, second) = join(first.connect(), second.connect()).await;
    let first = started(first, "replica 1");
    let second = started(second, "replica 2");
    assert_eq!(probe.runs(), 1, "two replicas racing on one server applied the migrations {} times", probe.runs());
    closed(first, "replica 1").await;
    closed(second, "replica 2").await;
}

fn started(outcome: Result<App<Connected>, StartupError>, what: &str) -> App<Connected> {
    outcome.unwrap_or_else(|error| {
        panic!("{what} did not connect beside the other{}: {}", migration_failure(&error), report(&error))
    })
}

/// Names a `MigrationFailed` under `error`, the refusal a replica gets that ran the migrations
/// unlocked into the other's, or that waited past the lock's bound.
fn migration_failure(error: &StartupError) -> String {
    match carried::<MigrationFailed>(error) {
        Some(failed) => format!(" (its migrations failed in `{}`)", failed.module),
        None => String::new(),
    }
}
