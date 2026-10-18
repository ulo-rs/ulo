use crate::Host;

/// Connecting does not stall a one-thread runtime, run through `Harness::block_on_one_thread` and
/// held to [`Budget::stall`](crate::Budget::stall). Planted break: a client created on the worker
/// outside `spawn_with`.
pub async fn connecting_does_not_stall_the_thread<H: Host>() {
    todo!()
}
