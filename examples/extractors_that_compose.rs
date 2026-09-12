//! An extractor built out of other extractors.
//!
//! `AuthContext` runs three of them and assembles one value, so a handler
//! takes a single parameter instead of three. Cookie parsing is the same
//! shape one level down: `SessionCookie` reads the map `Cookies` produced
//! rather than parsing the header again.
//!
//! Composition is ordinary function calls — an extractor is an `async fn` over
//! the context, and calling one from another needs no framework support.
//!
//!     cargo run --example extractors_that_compose

use std::collections::HashMap;
use std::fmt;

use serde::Deserialize;
use ulo::Body;
use ulo::http::HttpContext;
use ulo::http::extract::Json;
use ulo::{FromContext, controller, get, module, post, routes};

/// ## 7. Cookies Extractor
///
/// Parses Cookie header into a HashMap.
/// In NestJS, you'd use cookie-parser middleware first.

#[derive(Debug, Clone)]
pub struct Cookies(pub HashMap<String, String>);

#[derive(Debug)]
pub struct CookieError(String);

impl fmt::Display for CookieError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Cookie error: {}", self.0)
    }
}

impl std::error::Error for CookieError {}

impl FromContext<HttpContext> for Cookies {
    type Error = CookieError;

    async fn extract(ctx: &HttpContext) -> Result<Self, Self::Error> {
        let parts = ctx.request();
        let cookie_header = parts
            .headers
            .get("cookie")
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| CookieError("No cookies present".to_string()))?;

        let mut cookies = HashMap::new();

        // Parse cookies: "key1=value1; key2=value2"
        for pair in cookie_header.split(';') {
            let mut parts = pair.trim().splitn(2, '=');
            if let (Some(key), Some(value)) = (parts.next(), parts.next()) {
                cookies.insert(key.to_string(), value.to_string());
            }
        }

        Ok(Cookies(cookies))
    }
}

/// ## 8. SingleCookie Extractor (with name parameter)
///
/// This shows how to create parameterized extractors.
/// In NestJS: `@Cookies('sessionId')`
/// In Ulo: We need a different approach since we can't pass parameters to extractors directly.
///
/// Solution: Create specific extractor types or use the Cookies extractor and extract manually.

#[derive(Debug, Clone)]
pub struct SessionCookie(pub String);

#[derive(Debug)]
pub struct SessionCookieError(String);

impl fmt::Display for SessionCookieError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Session cookie error: {}", self.0)
    }
}

impl std::error::Error for SessionCookieError {}

impl FromContext<HttpContext> for SessionCookie {
    type Error = SessionCookieError;

    async fn extract(ctx: &HttpContext) -> Result<Self, Self::Error> {
        let parts = ctx.request();
        let cookie_header = parts
            .headers
            .get("cookie")
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| SessionCookieError("No cookies present".to_string()))?;

        for pair in cookie_header.split(';') {
            let mut parts = pair.trim().splitn(2, '=');
            if let (Some(key), Some(value)) = (parts.next(), parts.next()) {
                if key == "session_id" || key == "sessionId" {
                    return Ok(SessionCookie(value.to_string()));
                }
            }
        }

        Err(SessionCookieError(
            "session_id cookie not found".to_string(),
        ))
    }
}

/// Two extractors already defined above, plus one header, assembled into a
/// single value.
///
/// `SessionCookie` is itself the pattern one level down — it runs `Cookies`
/// rather than parsing the header a second time. Nothing here is framework
/// machinery: `extract` is an `async fn` over the context, so calling one from
/// another is an ordinary call, and the only decision is what to do with the
/// inner error.
#[derive(Debug, Clone)]
pub struct RequestContext {
    pub session: String,
    pub cookie_count: usize,
    pub user_agent: String,
}

#[derive(Debug)]
pub struct RequestContextError(String);

impl fmt::Display for RequestContextError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "request context: {}", self.0)
    }
}

impl std::error::Error for RequestContextError {}

impl FromContext<HttpContext> for RequestContext {
    type Error = RequestContextError;

    async fn extract(ctx: &HttpContext) -> Result<Self, Self::Error> {
        // A required part: its failure is the whole extraction's failure.
        let SessionCookie(session) = SessionCookie::extract(ctx)
            .await
            .map_err(|e| RequestContextError(format!("session: {e}")))?;

        // An optional part: `Cookies` cannot fail here, and reading it a second
        // time costs one more parse of the same header.
        let cookie_count = Cookies::extract(ctx).await.map(|c| c.0.len()).unwrap_or(0);

        // A part with a default: absent is not an error for this field.
        let user_agent = ctx
            .request()
            .headers
            .get("user-agent")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("unknown")
            .to_string();

        Ok(RequestContext {
            session,
            cookie_count,
            user_agent,
        })
    }
}

#[controller("/session")]
pub struct SessionController {}

#[routes]
impl SessionController {
    /// Example 7: Extract all cookies
    #[get("/cookies")]
    fn get_cookies(&self, Cookies(cookies): Cookies) -> Body {
        Body::json(serde_json::json!(cookies))
    }

    /// Example 8: Extract specific cookie
    #[get("/session")]
    fn get_session(&self, SessionCookie(session_id): SessionCookie) -> Body {
        Body::json(serde_json::json!({
            "sessionId": session_id
        }))
    }
}

#[controller("/advanced")]
pub struct AdvancedController {}

#[routes]
impl AdvancedController {
    /// Several extractors in one signature: each runs independently, and the
    /// first failure answers before the handler is called.
    #[post("/audit")]
    fn audit(&self, SessionCookie(session): SessionCookie, Json(data): Json<AuditData>) -> Body {
        Body::json(serde_json::json!({
            "session": session,
            "action": data.action,
            "timestamp": data.timestamp
        }))
    }

    /// The composed one, doing the same work behind a single parameter.
    #[get("/context")]
    fn get_context(&self, context: RequestContext) -> Body {
        Body::json(serde_json::json!({
            "session": context.session,
            "cookieCount": context.cookie_count,
            "userAgent": context.user_agent
        }))
    }
}

#[derive(Debug, Deserialize)]
pub struct AuditData {
    action: String,
    timestamp: i64,
}

#[module(controllers: [SessionController, AdvancedController])]
impl AppModule {}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut app = ulo::UloFactory::create(AppModule).await?;
    app.use_http_adapter(ulo_http_axum::AxumAdapter::new(), ("127.0.0.1", 3000))?;
    println!("listening on http://127.0.0.1:3000");
    app.start().await?;
    Ok(())
}
