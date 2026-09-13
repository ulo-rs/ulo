//! Everything that is HTTP and nothing that is not: the request and response, the body, the
//! method, the context one request runs in, the error it answers with, the adapter trait an
//! integration crate implements, and the request-scoped provider a handler injects.
//!
//! What an HTTP handler shares with the other transports — `Guard`, `Interceptor`, `FromContext`,
//! `ExecutionResult` — is in the crate's core, because it means the same thing there.

mod adapter;
mod body;
mod context;
pub mod extract;
mod lifecycle;
pub mod middleware;
mod provider;
mod request_handler;
mod route;
#[cfg(feature = "tower-compat")]
pub mod tower;
pub use self::adapter::HttpAdapter;
pub use self::body::{Body, BoxBody};
pub use self::context::HttpContext;
pub(crate) mod error;
pub use self::error::{HttpError, http_reason, http_status};
pub use self::lifecycle::HttpLifecycleHandle;
pub use self::provider::{Request, RequestFactory};
pub use self::request_handler::RequestHandler;
pub use self::route::{ControllerEnhancers, Route};

mod http_response;
pub use self::http_response::{HttpResponse, HttpResponseBuilder};

mod path_params;
pub use self::path_params::PathParams;

mod request_body;
pub use self::request_body::{RequestBody, RequestBoxBody};

mod http_request;
pub use self::http_request::{HttpRequest, RequestPart};

mod http_method;
pub use self::http_method::HttpMethod;

mod into_response;
pub use self::into_response::IntoResponse;

mod sse;
pub use self::sse::{Sse, SseEvent, sse};

/// Join a controller's route prefix with a handler's sub-path, normalizing slashes.
///
/// The `#[controller]` prefix lives on the struct and the sub-path on the `#[routes]` handler, so the
/// full path is composed at route-registration time rather than baked in by the macro.
///
/// Trailing slashes are insignificant: the joined path never carries one (except the root `/`),
/// and [`AdapterContext`](crate::spi::AdapterContext) trims them from incoming request paths, so
/// `/app` and `/app/` address the same route.
///
/// `"/api" + "/users"` → `"/api/users"`; `"/" + "/x"` → `"/x"`; `"/api" + ""` → `"/api"`;
/// `"/api" + "/users/"` → `"/api/users"`.
pub fn join_route(prefix: &str, sub_path: &str) -> String {
    let prefix = prefix.trim_end_matches('/');
    let sub_path = sub_path.trim_matches('/');
    if prefix.is_empty() {
        format!("/{}", sub_path)
    } else if sub_path.is_empty() {
        prefix.to_string()
    } else {
        format!("{}/{}", prefix, sub_path)
    }
}

/// Trim trailing slashes from a path, preserving the root `/`.
pub(crate) fn trim_trailing_slashes(path: &str) -> &str {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() { "/" } else { trimmed }
}

#[cfg(test)]
mod join_route_tests {
    use super::{join_route, trim_trailing_slashes};

    #[test]
    fn joined_paths_never_carry_a_trailing_slash() {
        assert_eq!(join_route("/app", "/"), "/app");
        assert_eq!(join_route("/app", ""), "/app");
        assert_eq!(join_route("/app", "/users/"), "/app/users");
        assert_eq!(join_route("/app/", "/users"), "/app/users");
        assert_eq!(join_route("/", "/x"), "/x");
        assert_eq!(join_route("/", "/"), "/");
        assert_eq!(join_route("", ""), "/");
    }

    #[test]
    fn trim_preserves_root_and_inner_slashes() {
        assert_eq!(trim_trailing_slashes("/app/"), "/app");
        assert_eq!(trim_trailing_slashes("/app///"), "/app");
        assert_eq!(trim_trailing_slashes("/a/b"), "/a/b");
        assert_eq!(trim_trailing_slashes("/"), "/");
        assert_eq!(trim_trailing_slashes("///"), "/");
    }
}
