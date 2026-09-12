//! An extractor that can fail, and what its error does.
//!
//! `FromContext::Error` is whatever the extractor names, and a failure is
//! rendered as a 400 without reaching the handler. Three here differ in what
//! they do when the header is missing: the client IP falls back through
//! `X-Forwarded-For` to the peer address, the user agent refuses, and the
//! request id mints one.
//!
//!     cargo run --example extractors_for_request_metadata

use std::fmt;
use ulo::context::HttpContext;
use ulo::http_helpers::Body;
use ulo::{FromContext, controller, get, module, routes};

/// ## 4. ClientIp Extractor
///
/// Extracts client IP, respecting X-Forwarded-For for proxies.
/// Useful for rate limiting, geolocation, or security logging.

#[derive(Debug, Clone)]
pub struct ClientIp(pub String);

#[derive(Debug)]
pub struct IpError(String);

impl fmt::Display for IpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Cannot extract IP address: {}", self.0)
    }
}

impl std::error::Error for IpError {}

impl FromContext<HttpContext> for ClientIp {
    type Error = IpError;

    async fn extract(ctx: &HttpContext) -> Result<Self, Self::Error> {
        let parts = ctx.request();
        // Check X-Forwarded-For first (proxy/load balancer support)
        if let Some(forwarded) = parts
            .headers
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
        {
            if let Some(ip) = forwarded.split(',').next() {
                return Ok(ClientIp(ip.trim().to_string()));
            }
        }

        // Check X-Real-IP (nginx)
        if let Some(real_ip) = parts.headers.get("x-real-ip").and_then(|v| v.to_str().ok()) {
            return Ok(ClientIp(real_ip.to_string()));
        }

        // Fallback (in real app, you'd extract from socket connection)
        Err(IpError("No IP address headers found".to_string()))
    }
}

/// ## 5. UserAgent Extractor
///
/// Extracts User-Agent string for analytics or compatibility checks.

#[derive(Debug, Clone)]
pub struct UserAgent(pub String);

#[derive(Debug)]
pub struct UserAgentError(String);

impl fmt::Display for UserAgentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for UserAgentError {}

impl FromContext<HttpContext> for UserAgent {
    type Error = UserAgentError;

    async fn extract(ctx: &HttpContext) -> Result<Self, Self::Error> {
        let parts = ctx.request();
        let ua = parts
            .headers
            .get("user-agent")
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| UserAgentError("User-Agent header missing".to_string()))?
            .to_string();

        Ok(UserAgent(ua))
    }
}

/// ## 6. RequestId Extractor
///
/// Extracts or generates request ID for distributed tracing.
/// Used with logging middleware for request correlation.

#[derive(Debug, Clone)]
pub struct RequestId(pub String);

#[derive(Debug)]
pub struct RequestIdError;

impl fmt::Display for RequestIdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Request ID extraction failed")
    }
}

impl std::error::Error for RequestIdError {}

impl FromContext<HttpContext> for RequestId {
    type Error = RequestIdError;

    async fn extract(ctx: &HttpContext) -> Result<Self, Self::Error> {
        let parts = ctx.request();
        // Check existing header
        if let Some(id) = parts
            .headers
            .get("x-request-id")
            .and_then(|v| v.to_str().ok())
        {
            return Ok(RequestId(id.to_string()));
        }

        // Generate new ID (in real app, use uuid crate)
        let id = format!(
            "req_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis()
        );

        Ok(RequestId(id))
    }
}

#[controller("/metadata")]
pub struct MetadataController {}

#[routes]
impl MetadataController {
    /// Example 4: Extract client IP
    #[get("/ip")]
    fn get_ip(&self, ClientIp(ip): ClientIp) -> Body {
        Body::text(format!("Your IP: {}", ip))
    }

    /// Example 5: Extract user agent
    #[get("/user-agent")]
    fn get_user_agent(&self, UserAgent(ua): UserAgent) -> Body {
        Body::json(serde_json::json!({
            "userAgent": ua
        }))
    }

    /// Example 6: Extract request ID for tracing
    #[get("/trace")]
    fn trace(&self, RequestId(id): RequestId) -> Body {
        Body::json(serde_json::json!({
            "requestId": id,
            "message": "Use this ID for request tracing"
        }))
    }
}

#[module(controllers: [MetadataController])]
impl AppModule {}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut app = ulo::UloFactory::create(AppModule).await?;
    app.use_http_adapter(ulo_http_axum::AxumAdapter::new(), ("127.0.0.1", 3000))?;
    println!("listening on http://127.0.0.1:3000");
    app.start().await?;
    Ok(())
}
