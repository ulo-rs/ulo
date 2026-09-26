//! A service registered through `add_service` is named in a warning at `bind()`.
//!
//! tonic dispatches such a service directly: ulo's guards, interceptors, error handlers and panic
//! recovery never run for it. Reflection and health are registered that way, and nothing at
//! startup said which services were. The adapter warns once per such service; a service that
//! came through DI produces no warning.

#![allow(dead_code)]

use std::sync::{Arc, Mutex};

use tracing_subscriber::Layer;
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::registry::LookupSpan;
use ulo::UloFactory;
use ulo::extract::Payload;
use ulo::grpc::extract::Inbound;
use ulo::tracing::{self, Subscriber};
use ulo_macros::{controller, grpc_methods, module, new};

use crate::common::NotServed;

/// Every `warn` event the gRPC adapter emits, each as its fields rendered `name=value`.
#[derive(Clone, Default)]
struct Warnings(Arc<Mutex<Vec<String>>>);

impl Warnings {
    fn lines(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }
}

impl<S: Subscriber + for<'a> LookupSpan<'a>> Layer<S> for Warnings {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        if *event.metadata().level() != tracing::Level::WARN
            || !event.metadata().target().starts_with("ulo_grpc")
        {
            return;
        }
        let mut line = String::new();
        event.record(
            &mut |field: &tracing::field::Field, value: &dyn std::fmt::Debug| {
                line.push_str(&format!("{}={:?} ", field.name(), value));
            },
        );
        self.0.lock().unwrap().push(line);
    }
}

mod warn_pb {
    tonic::include_proto!("ulo_test.orders");
}

use warn_pb::orders_server::{Orders, OrdersServer};

/// A service that came through DI: registered by the framework, inside the pipeline.
#[controller]
pub struct ThroughDi {}

#[grpc_methods(warn_pb::orders_server::Orders)]
impl ThroughDi {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    #[grpc_method]
    async fn create(
        &self,
        Payload(_req): Payload<warn_pb::CreateOrderRequest>,
    ) -> Result<warn_pb::CreateOrderResponse, NotServed> {
        Err(NotServed)
    }

    #[grpc_method]
    async fn bulk_create(
        &self,
        _inbound: Inbound<warn_pb::CreateOrderRequest>,
    ) -> Result<warn_pb::BulkCreateResponse, NotServed> {
        Err(NotServed)
    }

    #[grpc_stream]
    async fn watch_progress(
        &self,
        Payload(_req): Payload<warn_pb::WatchRequest>,
    ) -> Result<
        impl futures_util::Stream<Item = Result<warn_pb::ProgressEvent, NotServed>> + Send + 'static,
        NotServed,
    > {
        Ok(futures_util::stream::empty())
    }

    #[grpc_stream]
    async fn chat(
        &self,
        _inbound: Inbound<warn_pb::ChatMessage>,
    ) -> Result<
        impl futures_util::Stream<Item = Result<warn_pb::ChatMessage, NotServed>> + Send + 'static,
        NotServed,
    > {
        Ok(futures_util::stream::empty())
    }
}

#[module(controllers: [ThroughDi])]
impl ThroughDiModule {}

/// Bind an application carrying `adapter`, collecting the adapter's warnings, then close it.
async fn bind_and_collect(adapter: ulo_grpc::GrpcAdapter) -> Vec<String> {
    let warnings = Warnings::default();
    let subscriber = tracing_subscriber::registry().with(warnings.clone());
    // Thread-local, so it outranks the default logger `create` installs globally; the test
    // runtime is single-threaded, so `bind` runs on this thread.
    let _guard = tracing::subscriber::set_default(subscriber);

    let mut app = UloFactory::create(ThroughDiModule).await.unwrap();
    app.use_grpc_adapter(adapter).unwrap();
    let _bound = app.bind().await.unwrap();
    app.close().await;

    warnings.lines()
}

#[tokio::test]
async fn a_service_registered_through_add_service_is_named_in_a_warning() {
    let (_, health) = tonic_health::server::health_reporter();
    let adapter = ulo_grpc::GrpcAdapter::new("127.0.0.1:0".parse().unwrap()).add_service(health);

    let lines = bind_and_collect(adapter).await;

    assert_eq!(
        lines.len(),
        1,
        "one warning per bypassing service: {lines:?}"
    );
    assert!(
        lines[0].contains("grpc.health.v1.Health") && lines[0].contains("add_service"),
        "{lines:?}"
    );
}

#[tokio::test]
async fn a_service_that_came_through_di_produces_no_warning() {
    let adapter = ulo_grpc::GrpcAdapter::new("127.0.0.1:0".parse().unwrap());

    let lines = bind_and_collect(adapter).await;

    assert!(lines.is_empty(), "{lines:?}");
}
