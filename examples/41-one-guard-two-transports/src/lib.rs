//! One guard type serving two transports: the same `Guard<T>` impl, generic over the transport,
//! guards an HTTP route and an RPC message, and each transport renders the refusal its own way.

use ulo::{App, AppBuilder, BoxError, ExecutionRef, Guard, Transport, injectable, module, routes};

/// Declared with `#[meta(..)]` on a handler that is shut.
pub struct Shut;

/// Refuses any handler declaring `Shut`. `AsRef<ExecutionRef>` is what every transport's context
/// implements, so nothing here names HTTP or RPC.
#[injectable]
pub struct Maintenance;

impl<T: Transport> Guard<T> for Maintenance
where
    T::Cx: AsRef<ExecutionRef>,
{
    async fn can_activate(&self, cx: &T::Cx) -> Result<bool, BoxError> {
        let exec: &ExecutionRef = cx.as_ref();
        Ok(exec.handler().and_then(|handler| handler.meta::<Shut>()).is_none())
    }
}

#[injectable]
pub struct ShopHttp;

#[routes]
#[guards(Maintenance)]
impl ShopHttp {
    #[ulo_http::get("/shop")]
    fn open(&self) -> &'static str {
        "open"
    }

    #[ulo_http::get("/shop/stocktake")]
    #[meta(Shut)]
    fn shut(&self) -> &'static str {
        "unreachable"
    }
}

#[injectable]
pub struct ShopRpc;

#[routes]
#[guards(Maintenance)]
impl ShopRpc {
    #[ulo_rpc::message("shop.open")]
    async fn open(&self) -> &'static str {
        "open"
    }

    #[ulo_rpc::message("shop.stocktake")]
    #[meta(Shut)]
    async fn shut(&self) -> &'static str {
        "unreachable"
    }
}

#[module(providers = [Maintenance], controllers = [ShopHttp, ShopRpc])]
pub struct ShopModule;

pub fn builder() -> AppBuilder {
    App::builder(ShopModule)
}
