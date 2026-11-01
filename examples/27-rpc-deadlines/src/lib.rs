use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde::{Deserialize, Serialize};
use ulo::{App, AppBuilder, Dep, ExecutionRef, Timer, injectable, module, routes};
use ulo_rpc::{Payload, RpcClient, RpcClientModule, RpcError};
use ulo_rpc_tcp::Tcp;

#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct Quote {
    pub sku: String,
    /// What the callee's own execution had left when the handler ran.
    pub left_ms: u64,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct Relayed {
    pub gateway_left_ms: u64,
    pub quote: Quote,
}

/// How many times the callee's handler has run.
#[injectable]
pub struct Runs {
    #[injectable(default)]
    pub count: AtomicUsize,
}

#[injectable]
pub struct Catalog {
    runs: Dep<Runs>,
    timer: Dep<dyn Timer>,
}

#[routes]
impl Catalog {
    #[ulo_rpc::message("price.quote")]
    async fn quote(&self, sku: Payload<String>, exec: ExecutionRef) -> Quote {
        self.runs.count.fetch_add(1, Ordering::SeqCst);
        Quote { sku: sku.0, left_ms: left_ms(&exec, &*self.timer) }
    }
}

fn left_ms(exec: &ExecutionRef, timer: &dyn Timer) -> u64 {
    exec.deadline().map_or(u64::MAX, |at| at.saturating_duration_since(timer.now()).as_millis() as u64)
}

#[module(providers = [Runs], controllers = [Catalog])]
pub struct CalleeModule;

/// Names the pricing service's client where more than one is injected.
pub struct Pricing;

#[injectable]
pub struct Gateway {
    pricing: Dep<RpcClient, Pricing>,
    timer: Dep<dyn Timer>,
}

impl Gateway {
    /// The call carries what is left of `exec`'s deadline to the callee, and does not leave while
    /// `exec` has none left.
    pub async fn quote(&self, exec: &ExecutionRef, sku: &str) -> Result<Quote, RpcError> {
        self.pricing.request::<_, Quote>("price.quote", sku).within(exec).await
    }
}

#[routes]
impl Gateway {
    #[ulo_rpc::message("gateway.quote")]
    async fn relay(&self, sku: Payload<String>, exec: ExecutionRef) -> Result<Relayed, RpcError> {
        Ok(Relayed { gateway_left_ms: left_ms(&exec, &*self.timer), quote: self.quote(&exec, &sku.0).await? })
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
#[module(
    imports = [RpcClientModule::for_root(Tcp::new(self.callee)).keyed::<Pricing>()],
    controllers = [Gateway],
)]
pub struct GatewayModule {
    pub callee: SocketAddr,
}

pub fn callee() -> AppBuilder {
    App::builder(CalleeModule)
}

pub fn gateway(callee: SocketAddr) -> AppBuilder {
    App::builder(GatewayModule { callee })
}
