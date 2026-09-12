mod chain;
mod cors;
mod route_pattern;
mod traits;
pub use chain::MiddlewareChain;
pub use cors::{AllowedOrigins, CorsMiddleware, CorsOptions};
pub use route_pattern::{IntoRoutePattern, RoutePattern};

mod module_middleware;
pub use module_middleware::MiddlewareManager;

// Re-export core traits
pub(crate) use traits::NextInternal;
pub use traits::{
    FunctionalMiddleware, Middleware, MiddlewareConfiguration, MiddlewareFn, MiddlewareResult,
    NextHandle,
};
