use serde_json::Value;

use super::{Body, HttpResponse};
use crate::dispatch::{IntoOutput, transport::Http};

/// Converts a value into an [`HttpResponse`].
///
/// Implement this to make a type returnable from a controller handler.
/// All built-in types (`Body`, `String`, `&str`, `serde_json::Value`, etc.)
/// are already covered.
///
/// This is HTTP's spelling of [`IntoOutput<Http>`](crate::dispatch::IntoOutput), which every
/// implementor gets through the blanket below. A handler's `Result` is served by
/// `IntoOutput`'s own impl and does not go through here, which is what keeps a returned error on
/// the error path instead of rendering it.
pub trait IntoResponse {
    fn into_response(self) -> HttpResponse;
}

impl<T: IntoResponse> IntoOutput<Http> for T {
    fn into_output(self) -> crate::dispatch::transport::Answer<Http> {
        Ok(self.into_response())
    }
}

impl IntoResponse for HttpResponse {
    fn into_response(self) -> HttpResponse {
        self
    }
}

impl IntoResponse for Body {
    fn into_response(self) -> HttpResponse {
        HttpResponse {
            body: Some(self),
            ..HttpResponse::new()
        }
    }
}

impl IntoResponse for u16 {
    fn into_response(self) -> HttpResponse {
        HttpResponse {
            status: self,
            ..HttpResponse::new()
        }
    }
}

impl IntoResponse for Vec<(String, String)> {
    fn into_response(self) -> HttpResponse {
        HttpResponse {
            headers: self,
            ..HttpResponse::new()
        }
    }
}

impl IntoResponse for (u16, Body) {
    fn into_response(self) -> HttpResponse {
        HttpResponse {
            status: self.0,
            body: Some(self.1),
            ..HttpResponse::new()
        }
    }
}

impl IntoResponse for Value {
    fn into_response(self) -> HttpResponse {
        HttpResponse {
            body: Some(Body::json(self)),
            ..HttpResponse::new()
        }
    }
}

impl IntoResponse for String {
    fn into_response(self) -> HttpResponse {
        HttpResponse {
            body: Some(Body::text(self)),
            ..HttpResponse::new()
        }
    }
}

impl IntoResponse for &'static str {
    fn into_response(self) -> HttpResponse {
        HttpResponse {
            body: Some(Body::text(self)),
            ..HttpResponse::new()
        }
    }
}
