use crate::Host;

/// Migrations run only on request. Planted break: migrations run without `.migrate(..)`.
pub async fn only_on_request<H: Host>() {
    todo!()
}

/// Two replicas started against one server apply the migrations once, the second waiting on the
/// lock. The scenario uses ordinary DDL, since `CREATE INDEX CONCURRENTLY` can deadlock against a
/// session waiting on an advisory lock. Planted break: the migration lock removed.
pub async fn once_across_replicas<H: Host>() {
    todo!()
}
