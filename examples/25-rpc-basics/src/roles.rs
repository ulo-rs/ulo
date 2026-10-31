use ulo::{BoxError, Guard, injectable};
use ulo_rpc::{Rpc, RpcCx};

/// Declared on a handler with `#[meta(..)]`; a guard reads it through the handler's info.
pub struct RequiresRole(pub &'static str);

/// Admits a call whose `x-role` header names the role the handler's `RequiresRole` declares. A
/// handler declaring none is open. Written as example 16's guard is, over `Rpc` instead of `Http`.
#[injectable]
pub struct RoleGuard;

impl Guard<Rpc> for RoleGuard {
    async fn can_activate(&self, cx: &RpcCx) -> Result<bool, BoxError> {
        let needed = cx.exec().handler().and_then(|handler| handler.meta::<RequiresRole>()).map(|role| role.0);
        Ok(needed.is_none_or(|needed| cx.headers().get("x-role") == Some(needed)))
    }
}
