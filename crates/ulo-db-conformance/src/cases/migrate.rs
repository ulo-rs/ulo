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

/// Migrations run only on request. Planted breaks: migrations run without `.migrate(..)`; migrations
/// not finished when `connect` returns (the migration run in a detached task).
///
/// An app whose module has no `.migrate(..)` connects and closes first. An app whose module is
/// given the host's migrations through [`Host::migrating`] then wires without running them, since
/// `wire` builds nothing, and has run them once when `connect` returns, in the module's init hook:
/// had the first app applied them, nothing would be pending and the version table would stay empty.
/// The table is read through the probe, not counted by the migrations. A second app given the same
/// migrations finds nothing pending, and the table still holds one row. A migration run at
/// registration, or run again over a schema that holds it, moves the count off the one asserted;
/// a migration still running when `connect` returns reads as an empty table.
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
    assert_eq!(probe.applied().await, 0, "migrations ran at `wire`, which builds nothing");
    let app = match app.connect().await {
        Ok(app) => app,
        Err(error) => panic!("an app with `.migrate(..)` did not connect{}: {}", migration_failure(&error), report(&error)),
    };
    assert_eq!(
        probe.applied().await,
        1,
        "the requested migrations were not finished when `connect` returned: none pending means an app that did not ask for them applied them, and a migration left to a detached task is still running"
    );
    closed(app, "the app with `.migrate(..)`").await;

    let again = connected(
        builder::<H>(Root::new().import(host.migrating(store::<H>(host.options()), probe.clone()))),
        "a second app with `.migrate(..)`",
    )
    .await;
    assert_eq!(probe.applied().await, 1, "migrations already applied ran again in a second app");
    closed(again, "the second app with `.migrate(..)`").await;
}

/// Two replicas started against one server apply the migrations once, the second waiting on the
/// lock. The scenario uses ordinary DDL, since `CREATE INDEX CONCURRENTLY` can deadlock against a
/// session waiting on an advisory lock. Planted breaks: the migration lock removed; the lock
/// released before the migrations run.
///
/// The replicas are two apps in one process, each with its own graph and its own client, built
/// from the host's options with the same migrations, so the server sees two clients as it would
/// two processes. Both are wired first and then connected together, and the host's migrations hold
/// each replica at a two-party rendezvous (500ms at most) before the real migrator runs, so without
/// a lock both start together. Both connect, and the version table, read once after both have
/// finished, holds one row. Nothing is counted around a replica's own run: the migration is a
/// `CREATE TABLE`, which the server refuses to repeat, so a second application fails that
/// replica's start.
///
/// For sqlx the lock-removal break stays green: sqlx's own migrator takes a lock of its own, which
/// makes ulo's redundant there, so this scenario cannot prove ulo's lock on sqlx and the ordering
/// break (the lock released before the migrations run) is the evidence for sqlx. Lock removal is
/// the evidence for SeaORM and Diesel, whose migrators take none.
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
    let applied = probe.applied().await;
    assert_eq!(applied, 1, "the version table holds {applied} rows after two replicas raced on one server");
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
