use crate::Host;

/// `.lazy()` keeps readiness false until the first ping. Planted break: readiness set true before
/// the first ping.
pub async fn waits_for_the_first_ping<H: Host>() {
    todo!()
}
