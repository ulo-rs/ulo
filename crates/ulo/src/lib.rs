#[path = "adapter/mod.rs"]
pub mod adapter;
mod application_context;
pub mod builtin_module;
pub mod context;
mod error;
mod startup_check;
pub use context::{
    CancellationToken, GrpcContext, HandlerContext, HttpContext, RpcContext, StandaloneContext,
    WsContext,
};
#[doc(hidden)]
pub mod __construct;
#[doc(hidden)]
pub mod __detect;
#[doc(hidden)]
pub mod __dispatch;
#[doc(hidden)]
pub mod __lifecycle;
#[doc(hidden)]
pub mod __rpc;
#[doc(hidden)]
pub mod __ws;
pub mod di;
pub mod errors;
pub mod extractors;
pub mod grpc_status;
pub use grpc_status::{GrpcCode, GrpcHandlerResult, GrpcStatus};
mod extension;
pub mod grpc_runtime;
pub mod http_helpers;
pub mod injector;
pub mod middleware;
pub mod module_helpers;
pub mod panic_recovery;
pub mod provider_scope;
mod request;
mod router;
pub mod rpc;
mod scanner;
mod structs_helpers;
pub mod traits_helpers;
pub mod type_map;
pub mod ulo_application;
pub mod ulo_factory;
pub mod websocket;

// Re-exported for use in macro-generated code — not part of the public API.
#[doc(hidden)]
pub use tracing;

// Public re-export: macro-generated code builds error bodies with it, and
// `Body::json` traffics in its `Value` type, so consumers need it in scope
// without declaring their own dependency.
pub use serde_json;

// Re-exports for adapter crates
pub use adapter::{
    AdapterContext, BindTarget, GrpcAdapter, GrpcLifecycleHandle, HttpAdapter, HttpLifecycleHandle,
    MessageCallbackResult, RequestHandler, RpcAdapter, RpcClientTransport, RpcLifecycleHandle,
    RpcMessageCallbacks, WebSocketAdapter, WsConnectionCallbacks, WsLifecycleHandle,
};
pub use http_helpers::{
    Body, BoxBody, HttpMethod, HttpRequest, HttpResponse, HttpResponseBuilder, IntoResponse,
    RequestBody, RequestBoxBody, RequestPart, Sse, SseEvent, sse,
};
pub use injector::InstanceWrapper;
pub use rpc::{
    RpcCallInfo, RpcClient, RpcClientError, RpcControllerSource, RpcControllerTrait, RpcData,
    RpcEnhancers, RpcError, RpcHandlerEnhancers, RpcHandlerOutput, RpcHandlerResult,
    RpcReplyStream,
};
pub use websocket::{
    BroadcastError, BroadcastModule, BroadcastService, BroadcastTarget, ClientId, DisconnectReason,
    GatewayEnhancers, GatewayHandlerEnhancers, GatewayTrait, GatewayWrapper, RoomId, SendError,
    Session, TrySendError, WsClient, WsError, WsHandlerOutput, WsHandlerResult, WsHandshake,
    WsMessage, WsSink,
};

// Re-export built-in providers
pub use extension::{Extension, ExtensionFactory};
pub use request::{Request, RequestFactory};

// Re-export ModuleRef for dynamic DI resolution
pub use injector::{IntoToken, ModuleRef};

pub use application_context::UloApplicationContext;

// Re-export dependencies used in macro-generated code
// This allows users to only depend on `ulo` without needing to add these explicitly
pub use async_trait::async_trait;
/// Re-exported for generated code: a streaming gRPC handler's reply is boxed
/// and mapped in the expansion, which needs a `Stream` path the user's crate
/// can name without depending on `futures` itself.
pub use futures;
pub use rustc_hash::FxHashMap;

// Re-export provider scope
pub use provider_scope::ProviderScope;

pub use traits_helpers::{ExecutionCache, ModuleMetadata, ProviderContext};

pub use error::{AdapterResult, InitResult, ResolutionError, StartupError};
pub use errors::{
    Error, ErrorKind, GuardRejection, HttpError, MiddlewareFailure, PanicRecovered, PipelineSegment,
};
pub use startup_check::StartupCheck;

// Re-export trait so users wont have to import manually
pub use extractors::{BodyStream, FromContext, take_body};

// Re-export macros
pub use ulo_macros::*;

pub use module_helpers::{CheckedModule, DynamicModule, ModuleIdentity};
pub use ulo_application::{BoundAdapters, ShutdownHandle, UloApplication};
pub use ulo_factory::UloFactory;

#[cfg(feature = "tower-compat")]
pub mod tower_compat;
#[cfg(feature = "tower-compat")]
pub use tower_compat::TowerLayer;
