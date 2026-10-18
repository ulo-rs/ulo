use crate::Host;

/// The health check goes down in an outage and recovers with the server. Planted break: a check that
/// reports a cached `Up`.
pub async fn outage_and_recovery<H: Host>() {
    todo!()
}
