use crate::Host;

/// An unsupported runtime is refused at `connect`, `Permanent`, as `UnsupportedRuntime`. Planted
/// break: the name check removed.
pub async fn unsupported_runtime_is_refused<H: Host>() {
    todo!()
}
