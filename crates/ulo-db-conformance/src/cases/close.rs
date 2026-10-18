use crate::Host;

/// The pool closes after the transports drain, a query in flight at the drain completing. Planted
/// break: a pool closed before the drain.
pub async fn after_the_drain<H: Host>() {
    todo!()
}
