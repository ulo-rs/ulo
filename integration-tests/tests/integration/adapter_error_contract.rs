//! An adapter's own error reaches the caller intact.
//!
//! The SPI answers with `AdapterResult`, which names no cases: `bind` wraps whatever an adapter
//! returns into `StartupError::Adapter` with the transport attached and reads nothing else off it.
//! What that has to be worth is the error surviving the trip — a caller that knows the adapter's
//! error type recovers it from the `source`, and one that does not still reads the message.

use std::error::Error;
use std::sync::Arc;

use ulo::rpc::RpcContext;
use ulo::rpc::{RpcData, RpcError};
use ulo::{
    AdapterResult, RpcAdapter, RpcLifecycleHandle, RpcMessageCallbacks, StartupError, UloFactory,
    async_trait, module,
};
use ulo_macros::{controller, message_pattern, new, patterns};

/// The kind of error an adapter crate writes for itself, on std alone.
#[derive(Debug)]
struct BrokerUnreachable {
    endpoint: String,
}

impl std::fmt::Display for BrokerUnreachable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "no broker reachable at {}", self.endpoint)
    }
}

impl Error for BrokerUnreachable {}

struct TypedFailureAdapter;

#[async_trait]
impl RpcAdapter for TypedFailureAdapter {
    fn register_handlers(
        &mut self,
        _patterns: &[String],
        _callbacks: Arc<RpcMessageCallbacks>,
    ) -> AdapterResult {
        Err(Box::new(BrokerUnreachable {
            endpoint: "broker:5672".to_string(),
        }))
    }

    async fn into_lifecycle(self: Box<Self>) -> AdapterResult<RpcLifecycleHandle> {
        unreachable!("registration refuses before a socket is asked for")
    }
}

/// An adapter with no error type of its own: a formatted string is the whole error.
struct UntypedFailureAdapter;

#[async_trait]
impl RpcAdapter for UntypedFailureAdapter {
    fn register_handlers(
        &mut self,
        _patterns: &[String],
        _callbacks: Arc<RpcMessageCallbacks>,
    ) -> AdapterResult {
        Err(format!("refused {} patterns", 3).into())
    }

    async fn into_lifecycle(self: Box<Self>) -> AdapterResult<RpcLifecycleHandle> {
        unreachable!("registration refuses before a socket is asked for")
    }
}

/// One pattern, so registration is reached and the adapter is asked to take it.
#[controller]
pub struct OrdersController {}

#[patterns]
impl OrdersController {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    #[message_pattern("orders.get")]
    async fn get(&self, data: RpcData, _ctx: &RpcContext) -> Result<RpcData, RpcError> {
        Ok(data)
    }
}

#[module(controllers: [OrdersController])]
struct AppModule;

async fn bind_with(adapter: impl RpcAdapter + 'static) -> StartupError {
    let mut app = UloFactory::create(AppModule).await.unwrap();
    app.use_rpc_adapter(adapter).unwrap();
    app.bind()
        .await
        .expect_err("the adapter refuses to register")
}

/// The concrete type an adapter raised is recoverable from the `source`, with its fields.
#[tokio::test]
async fn a_typed_adapter_error_survives_as_the_source() {
    let err = bind_with(TypedFailureAdapter).await;

    let StartupError::Adapter { transport, source } = &err else {
        panic!("an adapter failure is a StartupError::Adapter, got: {err:?}");
    };
    assert_eq!(*transport, "rpc");

    let broker = source
        .downcast_ref::<BrokerUnreachable>()
        .expect("the adapter's own type is what the source holds");
    assert_eq!(broker.endpoint, "broker:5672");
}

/// Nothing forces an adapter to own an error type; a string is a complete answer.
#[tokio::test]
async fn an_untyped_adapter_error_keeps_its_message() {
    let err = bind_with(UntypedFailureAdapter).await;

    let StartupError::Adapter { source, .. } = &err else {
        panic!("an adapter failure is a StartupError::Adapter, got: {err:?}");
    };
    assert_eq!(source.to_string(), "refused 3 patterns");
}

/// The transport name comes from the framework, not from the adapter, and reaches the rendering.
#[tokio::test]
async fn the_framework_names_the_transport_the_adapter_did_not() {
    let err = bind_with(TypedFailureAdapter).await;

    let rendered = err.to_string();
    assert!(
        rendered.contains("rpc") && rendered.contains("broker:5672"),
        "the rendering carries the framework's transport and the adapter's message, got: {rendered}"
    );
    assert!(
        err.source().is_some(),
        "the chain is walkable past StartupError"
    );
}
