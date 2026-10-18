use crate::Host;

/// A malformed URL is refused at `wire`, the password redacted. Planted break: a password echoed in
/// the refusal.
pub async fn malformed_url_is_refused<H: Host>() {
    todo!()
}
