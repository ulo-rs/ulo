//! Extractors beyond `Path`/`Query`/`Json`: `Form`, a request-side `BodyStream` and an extractor
//! written here, `Caller`, which implements `FromCall`.

use std::fmt;

use futures_util::StreamExt;
use serde::Deserialize;
use ulo::{BoxError, injectable, routes};
use ulo_http::{BodyStream, ExtractError, Form, FromCall, Http, HttpCx};

/// The caller named by `authorization: Bearer <name>`. `FromCall<Http>` is the whole contract: a
/// missing header is `Missing`, which `Option<Caller>` turns into `None`, and any other scheme is
/// `Malformed`, which it does not.
pub struct Caller(pub String);

#[derive(Debug)]
struct NotBearer;

impl fmt::Display for NotBearer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("the scheme is not Bearer")
    }
}

impl std::error::Error for NotBearer {}

impl FromCall<Http> for Caller {
    async fn from_call(cx: &HttpCx) -> Result<Self, ExtractError> {
        let param = "authorization";
        let value = cx.headers().get(param).ok_or(ExtractError::Missing { param })?;
        match value.to_str().ok().and_then(|text| text.strip_prefix("Bearer ")) {
            Some(name) => Ok(Caller(name.to_owned())),
            None => Err(ExtractError::Malformed { param, source: cx.app().redact(Box::new(NotBearer)) }),
        }
    }
}

#[derive(Deserialize)]
pub struct Login {
    pub user: String,
    pub pw: String,
}

#[injectable]
pub struct IntakeController;

#[routes]
impl IntakeController {
    #[ulo_http::post("/login")]
    fn login(&self, Form(login): Form<Login>) -> String {
        format!("{} ({} characters of password)", login.user, login.pw.chars().count())
    }

    /// Counts the body as it arrives, never holding it whole.
    #[ulo_http::post("/count")]
    async fn count(&self, mut body: BodyStream) -> Result<String, BoxError> {
        let (mut chunks, mut bytes) = (0, 0);
        while let Some(chunk) = body.next().await {
            chunks += 1;
            bytes += chunk?.len();
        }
        Ok(format!("{bytes} bytes in {chunks} chunks"))
    }

    #[ulo_http::get("/whoami")]
    fn whoami(&self, Caller(name): Caller) -> String {
        name
    }

    #[ulo_http::get("/whoami-or-guest")]
    fn whoami_or_guest(&self, caller: Option<Caller>) -> String {
        caller.map_or_else(|| "guest".to_owned(), |Caller(name)| name)
    }
}
