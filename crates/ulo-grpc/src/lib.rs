// Tests: this crate has no `tests/` of its own. Nothing it does is
// observable without an application dispatching through it, so its
// behaviour is proved in `integration-tests`:
// the fourteen `grpc_*` files, plus `rpc_grpc*` for the macro forms.

//! gRPC transport adapter for the [Ulo](https://github.com/ulo-rs/ulo) framework.
//!
//! Drives a [`tonic`](https://docs.rs/tonic) server through Ulo's bind /
//! serve / drain lifecycle. Services declared with the framework's
//! `#[controller]` + `#[grpc_methods]` macros are discovered via DI and
//! wrapped with the enhancer pipeline (guards / interceptors / error
//! handlers + panic recovery) before being handed to tonic — there is no
//! manual `*Server::new(handler)` step in user code.
//!
//! # Minimal example
//!
//! The build script compiles the proto and writes what each method carries
//! beside the trait, which is what lets a handler's parameters be extractors:
//!
//! ```ignore
//! // build.rs
//! ulo_build::compile_protos("proto/orders.proto")?;
//! ```
//!
//! ```ignore
//! use std::net::SocketAddr;
//! use ulo::UloFactory;
//! use ulo::extract::Payload;
//! use ulo_macros::{controller, grpc_methods, module, new};
//!
//! mod orders_pb {
//!     tonic::include_proto!("ulo_examples.orders");
//! }
//!
//! #[controller]
//! pub struct OrdersGrpcService {}
//!
//! #[grpc_methods(orders_pb::orders_server::Orders)]
//! impl OrdersGrpcService {
//!     #[new]
//!     pub fn new() -> Self { Self {} }
//!
//!     #[grpc_method]
//!     async fn create(&self, Payload(req): Payload<orders_pb::CreateOrderRequest>)
//!         -> Result<orders_pb::CreateOrderResponse, OrderError>
//!     { /* … */ }
//! }
//!
//! #[module(controllers: [OrdersGrpcService])]
//! struct AppModule;
//!
//! #[tokio::main]
//! async fn main() {
//!     let addr: SocketAddr = "127.0.0.1:50051".parse().unwrap();
//!     let mut app = UloFactory::create(AppModule).await.unwrap();
//!     app.use_grpc_adapter(ulo_grpc::GrpcAdapter::new(addr)).unwrap();
//!     app.start().await.unwrap();
//! }
//! ```
//!
//! # Mixing DI-registered and manually-added services
//!
//! [`GrpcAdapter::add_service`] accepts any tonic-generated service handle,
//! so a non-DI service can sit alongside the DI-registered ones:
//!
//! ```ignore
//! let adapter = ulo_grpc::GrpcAdapter::new(addr)
//!     .add_service(SomeOtherServer::new(other_handler));
//! app.use_grpc_adapter(adapter)?;
//! ```
//!
//! Services added this way do not get the enhancer pipeline — they pass
//! straight through to tonic. Services registered via DI go through guards,
//! interceptors, error handlers, and the panic catcher. `bind()` logs a `warn`
//! naming each service added this way; filter the `ulo_grpc` target to silence
//! it.
//!
//! # Reflection and health
//!
//! Both are ordinary services, so both arrive through `add_service` rather than
//! through anything this crate owns. Reflection lets `grpcurl` and its kin
//! explore the server without a local `.proto`:
//!
//! ```ignore
//! // build.rs — write the compiled schema somewhere the binary can read it
//! let descriptor = PathBuf::from(env::var("OUT_DIR")?).join("orders_descriptor.bin");
//! ulo_build::configure()
//!     .tonic(|b| b.file_descriptor_set_path(&descriptor))
//!     .compile_protos(&["proto/orders.proto"], &["proto"])?;
//!
//! // main.rs
//! const DESCRIPTOR: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/orders_descriptor.bin"));
//!
//! let reflection = tonic_reflection::server::Builder::configure()
//!     .register_encoded_file_descriptor_set(DESCRIPTOR)
//!     .build_v1()?;
//! let adapter = ulo_grpc::GrpcAdapter::new(addr).add_service(reflection);
//! ```
//!
//! `grpcurl -plaintext 127.0.0.1:50051 list` then answers with the services
//! the framework registered from `#[grpc_methods]`, since reflection and DI
//! discovery reach the same route set.
//!
//! Register the versions the callers need: newer tooling speaks
//! `grpc.reflection.v1`, older speaks `v1alpha`, and `build_v1alpha()` is the
//! other constructor. Which schemas a server exposes is a policy decision,
//! which is why it is written out rather than switched on.
//!
//! `tonic-health` works the same way — `tonic_health::server::health_reporter()`
//! hands back a service and a reporter, and the service goes to `add_service`.
//! # TLS
//!
//! [`GrpcAdapter::with_tls`] takes `tonic::transport::ServerTlsConfig` as it is.
//! Where certificates come from, how they rotate and whether client
//! certificates are demanded differ per deployment, so the configuration is
//! passed through rather than wrapped — mTLS is `ServerTlsConfig::client_ca_root`,
//! and the rest of the surface is tonic's.
//!
//! ```ignore
//! let identity = Identity::from_pem(fs::read("server.pem")?, fs::read("server.key")?);
//! let adapter = ulo_grpc::GrpcAdapter::new(addr)
//!     .with_tls(ServerTlsConfig::new().identity(identity));
//! ```
//!
//! The acceptor is built during `app.bind()`, so an unreadable certificate
//! fails startup rather than the first connection.
//!
//! Enable `tls-ring` or `tls-aws-lc` — the same crypto-provider choice tonic
//! asks for, left where the deployment can make it. With both enabled tonic
//! resolves to ring.
//!
//! # Drain timeout
//!
//! On shutdown the framework calls the adapter's `close`, which signals
//! tonic and starts a drain timer (default 10 s). When the timer elapses with
//! calls still in flight, their replies are ended: each closes with
//! `UNAVAILABLE`, the connections have nothing left to serve, and tonic's
//! graceful shutdown closes them. A streaming handler's cancellation token
//! fires as its reply ends. Closing is bounded by the same timer, so `close()`
//! returns within twice the drain timeout. Configure with
//! [`GrpcAdapter::with_drain_timeout`]; pass `None` to wait without bound.
//!
//! # Tracing
//!
//! Every dispatched method runs inside a `tracing::info_span!("rpc.request",
//! transport = "grpc", pattern = …, peer = …)` span, so any event the user
//! handler emits inherits those fields without having to thread context
//! through.
//!
//! See the crate's [README](https://docs.rs/ulo-grpc) for the full enhancer
//! API (guards / interceptors / error handlers / panic recovery) and a
//! runnable end-to-end example.

mod drain_layer;
mod grpc_adapter;
mod metadata;
mod method_path_layer;
mod reply;
pub mod shape;
mod tracing_layer;

pub use grpc_adapter::GrpcAdapter;
#[doc(hidden)]
pub use metadata::read_metadata;
pub use reply::{Envelope, reply};
pub use shape::{GrpcRequest, MethodShape};

/// Maps a domain error to a `tonic::Status` by its
/// [`kind`](ulo::Error::kind), the way every transport renders one, and
/// attaches the error to the status's source slot.
///
/// [`details`](ulo::Error::details) travels in `grpc-status-details-bin`, the
/// trailer the specification names for structured detail, as a
/// `google.rpc.Status` repeating the code and message and carrying the detail
/// as one `Any`: a JSON object packs as a `google.protobuf.Struct`, any other
/// JSON value as a `google.protobuf.Value`. An error with no detail writes no
/// trailer, and neither does a status whose code is `Ok`, which the
/// specification forbids the trailer on. `grpc-message` stays the text
/// description the specification defines it as. The trailer counts against a
/// client's trailer-size limit, which the specification suggests defaults to
/// 8 KiB.
///
/// A `#[grpc_methods]` handler returns its error and the generated method does
/// this. What is left for a caller is a service written against tonic's own
/// trait and registered through [`GrpcAdapter::add_service`], outside ulo's
/// dispatch — the orphan rule stops ulo implementing `From<E>` into a foreign
/// type, so the hop is written where both types are reachable:
///
/// ```ignore
/// async fn create(&self, request: Request<CreateOrderRequest>)
///     -> Result<Response<CreateOrderResponse>, Status>
/// {
///     let order = self.orders.create(request.into_inner()).map_err(ulo_grpc::to_status)?;
///     Ok(Response::new(order.into()))
/// }
/// ```
///
/// For a bare `?`, give the error type the impl in the crate that owns it:
///
/// ```ignore
/// impl From<OrderError> for tonic::Status {
///     fn from(e: OrderError) -> Self {
///         ulo_grpc::to_status(e)
///     }
/// }
/// ```
pub fn to_status<E: ulo::Error>(error: E) -> tonic::Status {
    to_tonic(ulo::grpc::GrpcStatus::of(error))
}

/// Fire the execution's cancellation token when the caller's deadline passes,
/// for as long as the execution lasts.
///
/// The `#[grpc_methods]` expansion calls this once per call, after building
/// the context. tonic 0.14 races the service call against `grpc-timeout`, and
/// that call resolves when the handler returns its response; for a streaming
/// method the body runs after that, outside the race. The timer runs past that
/// point: it fires the token at the deadline, which a producer feeding a
/// stream, or detached work holding a clone of the token, observes and stops
/// on. The timer ends with the execution's extensions, which drop with the
/// last clone of the context or of its `Extensions`: a unary call reaches that
/// when it returns, a stream when it ends. Work that keeps either clone past
/// the answer keeps the timer armed, and the token fires at the deadline. A
/// call with no deadline arms nothing.
pub fn arm_deadline(ctx: &ulo::grpc::GrpcContext) {
    use ulo::context::ExecutionContext;

    let Some(deadline) = ctx.deadline() else {
        return;
    };
    let token = ctx.cancellation().clone();
    let (alive, ended) = tokio::sync::oneshot::channel::<()>();
    // Dropped with the execution's extensions, which is what wakes `ended`.
    ctx.extensions().insert(ExecutionAlive(alive));
    tokio::spawn(async move {
        tokio::select! {
            _ = tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)) => token.cancel(),
            _ = ended => {}
        }
    });
}

/// Held in an execution's extensions for as long as the execution lasts; its
/// drop ends the deadline timer [`arm_deadline`] spawned.
struct ExecutionAlive(#[allow(dead_code)] tokio::sync::oneshot::Sender<()>);

/// The status a `GrpcStatus` renders as, keeping any error it carries on the
/// answer's source slot.
fn to_tonic(status: ulo::grpc::GrpcStatus) -> tonic::Status {
    let code = status.code as i32;
    let message = status.message.clone();
    let source = status.into_source();
    let details = source
        .as_ref()
        .filter(|_| code != 0)
        .and_then(|source| source.details());
    let mut answer = match details {
        Some(details) => tonic::Status::with_details(
            tonic::Code::from_i32(code),
            message.clone(),
            details_trailer(code, message, &details).into(),
        ),
        None => tonic::Status::new(tonic::Code::from_i32(code), message),
    };
    if let Some(source) = source {
        answer.set_source(std::sync::Arc::new(ulo::grpc::GrpcFailure::new(source)));
    }
    answer
}

/// The `google.rpc.Status` that `grpc-status-details-bin` carries, encoded.
///
/// The specification forbids the trailer's code to contradict `grpc-status`,
/// and has the consumer check it. The message repeats `grpc-message` for a
/// reader that takes both from the trailer.
fn details_trailer(code: i32, message: String, details: &serde_json::Value) -> Vec<u8> {
    use prost::Message;

    let detail = match details {
        serde_json::Value::Object(fields) => prost_types::Any {
            type_url: "type.googleapis.com/google.protobuf.Struct".to_string(),
            value: proto_struct(fields).encode_to_vec(),
        },
        other => prost_types::Any {
            type_url: "type.googleapis.com/google.protobuf.Value".to_string(),
            value: proto_value(other).encode_to_vec(),
        },
    };
    tonic_types::Status {
        code,
        message,
        details: vec![detail],
    }
    .encode_to_vec()
}

fn proto_struct(fields: &serde_json::Map<String, serde_json::Value>) -> prost_types::Struct {
    prost_types::Struct {
        fields: fields
            .iter()
            .map(|(key, value)| (key.clone(), proto_value(value)))
            .collect(),
    }
}

/// JSON's mapping onto `google.protobuf.Value`, which is the mapping protobuf's
/// own JSON form defines: every number becomes a double. A number outside a
/// double's range, which serde_json holds only under its `arbitrary_precision`
/// feature, travels as its decimal string, since protobuf's JSON form has no
/// non-finite number.
fn proto_value(value: &serde_json::Value) -> prost_types::Value {
    use prost_types::value::Kind;

    let kind = match value {
        serde_json::Value::Null => Kind::NullValue(prost_types::NullValue::NullValue as i32),
        serde_json::Value::Bool(b) => Kind::BoolValue(*b),
        serde_json::Value::Number(n) => n
            .as_f64()
            .filter(|f| f.is_finite())
            .map(Kind::NumberValue)
            .unwrap_or_else(|| Kind::StringValue(n.to_string())),
        serde_json::Value::String(s) => Kind::StringValue(s.clone()),
        serde_json::Value::Array(items) => Kind::ListValue(prost_types::ListValue {
            values: items.iter().map(proto_value).collect(),
        }),
        serde_json::Value::Object(fields) => Kind::StructValue(proto_struct(fields)),
    };
    prost_types::Value { kind: Some(kind) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ulo::grpc::{GrpcCode, GrpcStatus};

    #[derive(Debug)]
    struct Detailed;

    impl std::fmt::Display for Detailed {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("detailed")
        }
    }

    impl std::error::Error for Detailed {}

    impl ulo::Error for Detailed {
        fn kind(&self) -> ulo::ErrorKind {
            ulo::ErrorKind::BadRequest
        }

        fn details(&self) -> Option<serde_json::Value> {
            Some(serde_json::json!({"field": "qty"}))
        }
    }

    #[test]
    fn a_status_whose_code_is_ok_writes_no_trailer() {
        let ok = to_tonic(GrpcStatus::new(GrpcCode::Ok, "").caused_by(Detailed));
        assert!(ok.details().is_empty());

        let failed = to_tonic(GrpcStatus::new(GrpcCode::Aborted, "").caused_by(Detailed));
        assert!(!failed.details().is_empty());
    }

    #[test]
    fn a_finite_number_travels_as_a_double() {
        let value = proto_value(&serde_json::json!(1.5));
        assert_eq!(value.kind, Some(prost_types::value::Kind::NumberValue(1.5)));
    }
}
