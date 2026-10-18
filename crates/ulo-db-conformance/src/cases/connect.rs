use crate::Host;

/// An unreachable database fails `connect` at its bound, not `Permanent`. Planted break:
/// `Unreachable` treated as permanent.
pub async fn unreachable_fails_at_the_bound<H: Host>() {
    todo!()
}

/// Bad credentials fail `connect` at once, `Permanent`. Planted break: a permanent error retried to
/// the bound.
pub async fn bad_credentials_are_permanent<H: Host>() {
    todo!()
}

/// A store whose `create` connects retries `create` under the connect bound and stops at once on
/// `Permanent`; a store whose `create` connects nothing declares it not applicable. Planted break:
/// a `create` run once outside the loop.
pub async fn create_loop<H: Host>() {
    todo!()
}
