//! An RPC server and a client over MQTT.
//!
//! MQTT v5 carries request-response natively: a PUBLISH names a
//! `response_topic` and `correlation_data`, and per-call headers ride as user
//! properties.
//!
//! The handler side is the same code every transport runs — `#[controller]`
//! plus `#[patterns]`, with the pattern naming the topic. What changes
//! between transports is the adapter and the client transport, and nothing
//! else in this file would differ on NATS or TCP.
//!
//!     docker run -d -p 1883:1883 eclipse-mosquitto
//!     cargo run -p ulo-rpc-mqtt --example orders

use serde::{Deserialize, Serialize};
use ulo::UloFactory;
use ulo::rpc::RpcClient;
use ulo::rpc::{RpcData, RpcError};
use ulo_macros::{controller, module, new, patterns};
use ulo_rpc_mqtt::{MqttAdapter, MqttClientTransport};

#[derive(Debug, Deserialize)]
pub struct NewOrder {
    item: String,
    qty: u32,
}

#[derive(Debug, Serialize)]
pub struct Order {
    id: u64,
    item: String,
    qty: u32,
}

#[controller]
pub struct OrdersController {}

#[patterns]
impl OrdersController {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    /// Request-response: the caller waits for what this returns.
    #[message_pattern("order.create")]
    async fn create(&self, payload: RpcData) -> Result<RpcData, RpcError> {
        let order: NewOrder = payload
            .parse()
            .map_err(|e| RpcError::Internal(format!("bad payload: {e}")))?;

        if order.qty == 0 {
            return Err(RpcError::Internal("qty must be positive".into()));
        }

        RpcData::from_serialize(&Order {
            id: 1001,
            item: order.item,
            qty: order.qty,
        })
        .map_err(|e| RpcError::Internal(e.to_string()))
    }

    /// Fire-and-forget: no reply channel exists, so returning a value is not
    /// an option the signature offers.
    #[event_pattern("order.shipped")]
    async fn shipped(&self, payload: RpcData) -> Result<(), RpcError> {
        println!("shipped: {:?}", payload.as_json());
        Ok(())
    }
}

#[module(controllers: [OrdersController])]
impl AppModule {}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let endpoint = std::env::var("MQTT_HOST").unwrap_or_else(|_| "127.0.0.1".into());

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            let server = {
                let endpoint = endpoint.clone();
                tokio::task::spawn_local(async move {
                    let mut app = UloFactory::create(AppModule).await.unwrap();
                    app.use_rpc_adapter(MqttAdapter::new(endpoint.clone(), 1883))
                        .unwrap();
                    app.bind().await.unwrap();
                    println!("serving order.* over MQTT");
                    app.run().await;
                })
            };

            // Give the server its subscriptions before the first call.
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;

            let client = RpcClient::new(MqttClientTransport::new(endpoint.clone(), 1883));

            let created = client
                .send(
                    "order.create",
                    RpcData::from_serialize(&serde_json::json!({
                        "item": "keyboard",
                        "qty": 3
                    }))?,
                )
                .await?;
            println!("order.create -> {:?}", created.as_json());

            // An error from the handler comes back inside a successful frame,
            // as the canonical envelope rather than a transport failure.
            let refused = client
                .send(
                    "order.create",
                    RpcData::from_serialize(&serde_json::json!({
                        "item": "keyboard",
                        "qty": 0
                    }))?,
                )
                .await;
            println!("order.create (qty 0) -> {refused:?}");

            client
                .emit(
                    "order.shipped",
                    RpcData::from_serialize(&serde_json::json!({
                        "order_id": 1001
                    }))?,
                )
                .await?;
            println!("order.shipped emitted");

            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            server.abort();
            Ok::<_, anyhow::Error>(())
        })
        .await?;
    Ok(())
}
