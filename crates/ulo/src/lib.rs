// An item reachable from nowhere is still public API: it holds a name, appears in the rustdoc
// index, and cannot change without a major version. Denied rather than warned because CI's check
// job does not pass `-D warnings`, so a warning here would accumulate unnoticed.
#![deny(unreachable_pub)]

pub mod adapter;
mod application_context;
mod builtin_module;
pub mod context;
mod error;
mod startup_check;
pub use context::{CancellationToken, HandlerContext, HttpContext, RpcContext, StandaloneContext};
#[doc(hidden)]
pub mod __construct;
#[doc(hidden)]
pub mod __detect;
#[doc(hidden)]
pub mod __dispatch;
#[doc(hidden)]
pub mod __enhancer;
#[doc(hidden)]
pub mod __grpc;
#[doc(hidden)]
pub mod __lifecycle;
#[doc(hidden)]
pub mod __rpc;
#[doc(hidden)]
pub mod __ws;
mod application;
pub mod di;
mod enhancer;
pub mod errors;
mod extension;
pub mod extractors;
mod factory;
pub mod grpc;
pub use grpc::{
    GrpcAdapter, GrpcCode, GrpcContext, GrpcHandlerResult, GrpcLifecycleHandle, GrpcStatus,
};
mod http_types;
mod injector;
pub mod middleware;
mod modules;
mod panic_recovery;
mod provider_scope;
mod request;
mod router;
pub mod rpc;
mod scanner;
pub mod traits;
mod type_map;
pub mod ws;

// Re-exported for use in macro-generated code — not part of the public API.
#[doc(hidden)]
pub use tracing;

// Public re-export: macro-generated code builds error bodies with it, and
// `Body::json` traffics in its `Value` type, so consumers need it in scope
// without declaring their own dependency.
pub use serde_json;

// Re-exports for adapter crates
pub use adapter::{
    AdapterContext, BindTarget, HttpAdapter, HttpLifecycleHandle, RequestHandler, RpcAdapter,
    RpcClientTransport, RpcLifecycleHandle, RpcMessageCallbacks,
};
pub use http_types::{
    Body, BoxBody, HttpMethod, HttpRequest, HttpResponse, HttpResponseBuilder, IntoResponse,
    PathParams, RequestBody, RequestBoxBody, RequestPart, Sse, SseEvent, join_route, sse,
};
pub use rpc::{
    RpcCallInfo, RpcClient, RpcClientError, RpcController, RpcControllerSource, RpcData,
    RpcEnhancers, RpcError, RpcHandlerEnhancers, RpcHandlerOutput, RpcHandlerResult,
    RpcReplyStream,
};
pub use ws::{
    BroadcastError, BroadcastModule, BroadcastService, BroadcastTarget, ClientId, DisconnectReason,
    Gateway, GatewayEnhancers, GatewayHandlerEnhancers, MessageCallbackResult, RoomId, SendError,
    Session, TrySendError, WebSocketAdapter, WsClient, WsConnectionCallbacks, WsContext, WsError,
    WsHandlerOutput, WsHandlerResult, WsHandshake, WsLifecycleHandle, WsMessage, WsSink,
};

// Re-export built-in providers
pub use extension::{Extension, ExtensionFactory};
pub use request::{Request, RequestFactory};

// Re-export ModuleRef for dynamic DI resolution
pub use di::IntoToken;
pub use injector::ModuleRef;

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

pub use traits::{ExecutionCache, ModuleMetadata, ProviderContext};

pub use error::{AdapterResult, InitResult, ResolutionError, SetupResult, StartupError};
pub use errors::{
    Error, ErrorKind, GuardRejection, HttpError, MiddlewareFailure, PanicRecovered, PipelineSegment,
};
pub use startup_check::StartupCheck;

// Re-export trait so users wont have to import manually
pub use extractors::{BodyStream, FromContext, take_body};

// Re-export macros
pub use ulo_macros::*;

pub use application::{BoundAdapters, ShutdownHandle, UloApplication};
pub use factory::UloFactory;
pub use modules::{CheckedModule, DynamicModule, ModuleIdentity};

#[cfg(feature = "tower-compat")]
pub mod tower_compat;
#[cfg(feature = "tower-compat")]
pub use tower_compat::TowerLayer;
