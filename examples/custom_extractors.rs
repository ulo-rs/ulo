//! Writing an extractor: implement `FromContext` and a handler can take your
//! type as a parameter.
//!
//! Three that read the caller out of a request — a user resolved from a
//! bearer token, the raw token, an API key — and the same extractors wrapped
//! in `Option<T>`, which turns a refusal into `None` instead of a 401.
//!
//! An extractor is where ulo does what NestJS does with `createParamDecorator`,
//! and `validation_complete_guide.rs` carries that mapping in full.
//!
//!     cargo run --example custom_extractors

use std::fmt;

use serde::{Deserialize, Serialize};
use ulo::Body;
use ulo::http::HttpContext;
use ulo::{FromContext, controller, get, module, routes};

/// ## 1. CurrentUser Extractor
///
/// This is the most common custom decorator pattern - extracting the
/// authenticated user from the request after an auth guard/middleware
/// has validated the JWT and attached user data.

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: String,
    pub email: String,
    pub name: String,
    pub roles: Vec<String>,
}

/// Custom extractor wrapping User
#[derive(Debug, Clone)]
pub struct CurrentUser(pub User);

#[derive(Debug)]
pub enum AuthError {
    MissingToken,
    InvalidToken,
    ExpiredToken,
    Unauthorized,
}

impl fmt::Display for AuthError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AuthError::MissingToken => write!(f, "Authorization header missing"),
            AuthError::InvalidToken => write!(f, "Invalid JWT token format"),
            AuthError::ExpiredToken => write!(f, "JWT token has expired"),
            AuthError::Unauthorized => write!(f, "Insufficient permissions"),
        }
    }
}

impl std::error::Error for AuthError {}

impl FromContext<HttpContext> for CurrentUser {
    type Error = AuthError;

    async fn extract(ctx: &HttpContext) -> Result<Self, Self::Error> {
        let parts = ctx.request();
        let auth_header = parts
            .headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .ok_or(AuthError::MissingToken)?;

        let token = auth_header
            .strip_prefix("Bearer ")
            .ok_or(AuthError::InvalidToken)?;

        // In real app, use jsonwebtoken crate for JWT decoding
        let user = decode_jwt_token(token)?;

        Ok(CurrentUser(user))
    }
}

// Mock JWT decoder for example purposes
fn decode_jwt_token(token: &str) -> Result<User, AuthError> {
    if token.is_empty() {
        return Err(AuthError::InvalidToken);
    }

    if token == "expired" {
        return Err(AuthError::ExpiredToken);
    }

    Ok(User {
        id: "user123".to_string(),
        email: "user@example.com".to_string(),
        name: "John Doe".to_string(),
        roles: vec!["user".to_string()],
    })
}

/// ## 2. BearerToken Extractor
///
/// Extracts the raw JWT token without decoding it.
/// Useful for passing tokens to external services.

#[derive(Debug, Clone)]
pub struct BearerToken(pub String);

#[derive(Debug)]
pub enum TokenError {
    Missing,
    InvalidFormat,
}

impl fmt::Display for TokenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TokenError::Missing => write!(f, "Authorization header missing"),
            TokenError::InvalidFormat => write!(f, "Authorization header must be 'Bearer <token>'"),
        }
    }
}

impl std::error::Error for TokenError {}

impl FromContext<HttpContext> for BearerToken {
    type Error = TokenError;

    async fn extract(ctx: &HttpContext) -> Result<Self, Self::Error> {
        let parts = ctx.request();
        let auth_header = parts
            .headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .ok_or(TokenError::Missing)?;

        let token = auth_header
            .strip_prefix("Bearer ")
            .ok_or(TokenError::InvalidFormat)?
            .to_string();

        Ok(BearerToken(token))
    }
}

/// ## 3. ApiKey Extractor
///
/// Extracts API key from custom header.
/// Common for public APIs with key-based authentication.

#[derive(Debug, Clone)]
pub struct ApiKey(pub String);

#[derive(Debug)]
pub enum ApiKeyError {
    Missing,
    Invalid,
}

impl fmt::Display for ApiKeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ApiKeyError::Missing => write!(f, "X-API-Key header is required"),
            ApiKeyError::Invalid => {
                write!(f, "Invalid API key format (must be at least 32 characters)")
            }
        }
    }
}

impl std::error::Error for ApiKeyError {}

impl FromContext<HttpContext> for ApiKey {
    type Error = ApiKeyError;

    async fn extract(ctx: &HttpContext) -> Result<Self, Self::Error> {
        let parts = ctx.request();
        let key = parts
            .headers
            .get("x-api-key")
            .and_then(|v| v.to_str().ok())
            .ok_or(ApiKeyError::Missing)?
            .to_string();

        // Validate key format
        if key.len() < 32 {
            return Err(ApiKeyError::Invalid);
        }

        Ok(ApiKey(key))
    }
}

#[controller("/auth")]
pub struct AuthController {}

#[routes]
impl AuthController {
    /// Example 1: Extract authenticated user
    #[get("/profile")]
    fn get_profile(&self, CurrentUser(user): CurrentUser) -> Body {
        Body::json(serde_json::json!({
            "id": user.id,
            "email": user.email,
            "name": user.name,
            "roles": user.roles
        }))
    }

    /// Example 2: Extract bearer token
    #[get("/token")]
    fn get_token(&self, BearerToken(token): BearerToken) -> Body {
        Body::json(serde_json::json!({
            "token": token,
            "length": token.len()
        }))
    }
}

#[controller("/api")]
pub struct ApiController {}

#[routes]
impl ApiController {
    /// Example 3: Extract and validate API key
    #[get("/data")]
    fn get_data(&self, ApiKey(key): ApiKey) -> Body {
        Body::json(serde_json::json!({
            "message": "Authenticated with API key",
            "key_prefix": &key[..8]
        }))
    }
}

#[controller("/optional")]
pub struct OptionalController {}

#[routes]
impl OptionalController {
    /// Optional authentication - returns None when extraction fails instead of 400 error
    #[get("/feed")]
    fn get_feed(&self, user: Option<CurrentUser>) -> Body {
        if let Some(CurrentUser(user)) = user {
            Body::json(serde_json::json!({
                "type": "personalized",
                "message": format!("Welcome back, {}!", user.name),
                "items": ["Based on your interests", "Recommended for you"]
            }))
        } else {
            Body::json(serde_json::json!({
                "type": "public",
                "message": "Sign in for personalized content",
                "items": ["Popular posts", "Trending articles"]
            }))
        }
    }

    /// Multiple optional extractors - supports JWT, API key, or public access
    #[get("/data")]
    fn get_data(&self, user: Option<CurrentUser>, api_key: Option<ApiKey>) -> Body {
        if let Some(CurrentUser(user)) = user {
            return Body::json(serde_json::json!({
                "auth": "jwt",
                "userId": user.id
            }));
        }

        if let Some(ApiKey(key)) = api_key {
            return Body::json(serde_json::json!({
                "auth": "apiKey",
                "keyPrefix": &key[..8]
            }));
        }

        Body::json(serde_json::json!({
            "auth": "none",
            "message": "Public access (limited)"
        }))
    }
}

#[module(controllers: [AuthController, ApiController, OptionalController])]
impl AppModule {}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut app = ulo::UloFactory::create(AppModule).await?;
    app.use_http_adapter(ulo_http_axum::AxumAdapter::new(), ("127.0.0.1", 3000))?;
    println!("listening on http://127.0.0.1:3000");
    app.start().await?;
    Ok(())
}
