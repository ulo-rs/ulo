use std::net::SocketAddr;

use serde::{Deserialize, Serialize};
use tonic::Request;
use tonic::transport::Channel;
use ulo::{App, AppBuilder, BoxError, Dep, ExecutionRef, Module, ModuleDef, ModuleIdentity, injectable, routes};
use ulo_grpc::{GrpcClientModule, GrpcEndpoint, Message};
use ulo_http::{Json, Path};
use ulo_otel::Otel;
use ulo_rpc::{Payload, RpcClient, RpcClientModule};
use ulo_rpc_tcp::Tcp;

pub mod pb {
    ulo_grpc::include_proto!("pricing.v1");
}

use pb::pricing_client::PricingClient;

/// The `traceparent` of the execution a handler runs in.
fn traceparent(exec: &ExecutionRef) -> String {
    exec.trace().map(|trace| trace.traceparent()).unwrap_or_default()
}

/// Names the stock service's RPC client.
pub struct Stock;

/// Hop 3, gRPC: reports the context its own call carried.
#[injectable]
pub struct PricingService;

#[routes]
impl PricingService {
    #[ulo_grpc::method(pb::pricing::Quote)]
    async fn quote(&self, req: Message<pb::QuoteRequest>, exec: ExecutionRef) -> pb::Price {
        pb::Price { cents: 100 + req.0.sku.len() as u32, traceparent: traceparent(&exec) }
    }
}

#[derive(Serialize, Deserialize)]
pub struct Reserved {
    pub cents: u32,
    pub rpc_trace: String,
    pub grpc_trace: String,
}

/// Hop 2, RPC: calls Pricing from inside its own execution. `outgoing` writes the execution's
/// context into the call's metadata.
#[injectable]
pub struct StockService {
    pricing: Dep<PricingClient<Channel>>,
}

#[routes]
impl StockService {
    #[ulo_rpc::message("stock.reserve")]
    async fn reserve(&self, sku: Payload<String>, exec: ExecutionRef) -> Result<Reserved, BoxError> {
        let request = ulo_grpc::outgoing(&exec, Request::new(pb::QuoteRequest { sku: sku.0 }))?;
        let price = (*self.pricing).clone().quote(request).await?.into_inner();
        Ok(Reserved { cents: price.cents, rpc_trace: traceparent(&exec), grpc_trace: price.traceparent })
    }
}

#[derive(Serialize, Deserialize)]
pub struct Quoted {
    pub sku: String,
    pub cents: u32,
    pub http_trace: String,
    pub rpc_trace: String,
    pub grpc_trace: String,
}

/// Hop 1, HTTP: `.within(&exec)` writes the execution's context into the RPC call.
#[injectable]
pub struct EdgeService {
    stock: Dep<RpcClient, Stock>,
}

#[routes]
impl EdgeService {
    #[ulo_http::get("/quote/{sku}")]
    async fn quote(&self, Path(sku): Path<String>, exec: ExecutionRef) -> Result<Json<Quoted>, BoxError> {
        let reserved: Reserved = self.stock.request("stock.reserve", &sku).within(&exec).await?;
        Ok(Json(Quoted { sku, cents: reserved.cents, http_trace: traceparent(&exec), rpc_trace: reserved.rpc_trace, grpc_trace: reserved.grpc_trace }))
    }
}

/// One service of the chain: `otel` binds the `TraceSource` that opens a span per call served.
pub struct Service<F> {
    otel: Otel,
    register: F,
}

impl<F: Fn(&mut ModuleDef<'_>) + Send + Sync + 'static> Module for Service<F> {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.import(self.otel.module());
        (self.register)(m);
    }
}

pub fn pricing(otel: &Otel) -> AppBuilder {
    App::builder(Service { otel: otel.clone(), register: |m: &mut ModuleDef<'_>| { m.controller::<PricingService>(); } })
}

pub fn stock(otel: &Otel, pricing: SocketAddr) -> AppBuilder {
    App::builder(Service {
        otel: otel.clone(),
        register: move |m: &mut ModuleDef<'_>| {
            m.import(GrpcClientModule::<PricingClient<Channel>>::for_root(GrpcEndpoint::new(format!("http://{pricing}"))));
            m.controller::<StockService>();
        },
    })
}

pub fn edge(otel: &Otel, stock: SocketAddr) -> AppBuilder {
    App::builder(Service {
        otel: otel.clone(),
        register: move |m: &mut ModuleDef<'_>| {
            m.import(RpcClientModule::for_root(Tcp::new(stock)).keyed::<Stock>());
            m.controller::<EdgeService>();
        },
    })
}
