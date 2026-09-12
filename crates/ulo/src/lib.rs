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
pub use context::{CancellationToken, HandlerContext, StandaloneContext};
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
pub mod enhancer;
pub mod errors;
mod extension;
pub mod extract;
mod factory;
pub mod grpc;
pub use grpc::{
    GrpcAdapter, GrpcCode, GrpcContext, GrpcHandlerResult, GrpcLifecycleHandle, GrpcStatus,
};
pub mod http;
mod injector;
mod modules;
mod panic_recovery;
mod provider_scope;
mod router;
pub mod rpc;
mod scanner;
pub mod spi;
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
pub use adapter::{AdapterContext, BindTarget};
pub use http::{
    Body, BoxBody, HttpAdapter, HttpContext, HttpError, HttpLifecycleHandle, HttpMethod,
    HttpRequest, HttpResponse, HttpResponseBuilder, IntoResponse, PathParams, RequestBody,
    RequestBoxBody, RequestHandler, RequestPart, Sse, SseEvent, join_route, sse,
};
pub use rpc::{
    RpcAdapter, RpcCallInfo, RpcClient, RpcClientError, RpcClientTransport, RpcContext,
    RpcController, RpcControllerSource, RpcData, RpcEnhancers, RpcError, RpcHandlerEnhancers,
    RpcHandlerOutput, RpcHandlerResult, RpcLifecycleHandle, RpcMessageCallbacks, RpcReplyStream,
};
pub use ws::{
    BroadcastError, BroadcastModule, BroadcastService, BroadcastTarget, ClientId, DisconnectReason,
    Gateway, GatewayEnhancers, GatewayHandlerEnhancers, MessageCallbackResult, RoomId, SendError,
    Session, TrySendError, WebSocketAdapter, WsClient, WsConnectionCallbacks, WsContext, WsError,
    WsHandlerOutput, WsHandlerResult, WsHandshake, WsLifecycleHandle, WsMessage, WsSink,
};

// Re-export built-in providers
pub use extension::{Extension, ExtensionFactory};
pub use http::{Request, RequestFactory};

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

pub use di::{ExecutionCache, ModuleMetadata, ProviderContext};

pub use error::{AdapterResult, InitResult, ResolutionError, SetupResult, StartupError};
pub use errors::{
    Error, ErrorKind, GuardRejection, MiddlewareFailure, PanicRecovered, PipelineSegment,
};
pub use startup_check::StartupCheck;

// Re-export trait so users wont have to import manually
pub use extract::{FromContext, take_body};
pub use http::extract::BodyStream;

// Re-export macros
pub use ulo_macros::*;

pub use application::{BoundAdapters, ShutdownHandle, UloApplication};
pub use factory::UloFactory;
pub use modules::{CheckedModule, DynamicModule, ModuleIdentity};

#[cfg(feature = "tower-compat")]
pub use http::tower::TowerLayer;

/// What an application writes whatever it serves.
///
/// `use ulo::prelude::*` brings the bootstrap, the macros, the DI vocabulary, the enhancer traits
/// and the extraction traits — everything that means the same thing on HTTP, RPC, WebSocket and
/// gRPC. What a transport adds is its own: `ulo::http::{Body, HttpResponse}`,
/// `ulo::rpc::RpcData`, `ulo::ws::WsMessage`, `ulo::grpc::GrpcStatus`.
///
/// The prelude deliberately carries no transport's types. A framework that put HTTP's in here
/// would be saying HTTP is the default and the rest are extras, which is not how anything else in
/// the crate is arranged.
///
/// ```rust
/// use ulo::http::Body;
/// use ulo::prelude::*;
///
/// #[controller("/health")]
/// pub struct Health {}
///
/// #[routes]
/// impl Health {
///     #[get("/")]
///     async fn check(&self) -> Body {
///         Body::text("ok")
///     }
/// }
///
/// #[module(controllers: [Health])]
/// impl AppModule {}
/// ```
pub mod prelude {
    pub use crate::{BoundAdapters, ShutdownHandle, StartupCheck, StartupError, UloApplication};
    pub use crate::{UloApplicationContext, UloFactory};

    pub use crate::di::{ModuleMetadata, ProviderContext};
    pub use crate::{CheckedModule, DynamicModule, Extension, ModuleIdentity, ModuleRef};
    pub use crate::{InitResult, ProviderScope};

    pub use crate::enhancer::{ChainError, ErrorHandler, Guard, Interceptor, InterceptorNext};
    pub use crate::errors::{Error, ErrorKind};
    pub use crate::extract::{FromContext, Payload, Validated, take_body};

    pub use crate::async_trait;
    pub use ulo_macros::*;
}
