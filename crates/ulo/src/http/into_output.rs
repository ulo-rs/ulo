//! What an HTTP handler may answer with.
//!
//! One impl per returnable type, the same shape RPC and WebSocket use. A conversion answers
//! [`Answer<Http>`](crate::dispatch::Answer), so a value that cannot be rendered has somewhere to
//! say so and an error a handler returns stays on the error side.

use serde_json::Value;

use super::{Body, HttpResponse};
use crate::dispatch::{Answer, Http, IntoOutput};

impl IntoOutput<Http> for HttpResponse {
    fn into_output(self) -> Answer<Http> {
        Ok(self)
    }
}

impl IntoOutput<Http> for Body {
    fn into_output(self) -> Answer<Http> {
        Ok(HttpResponse {
            body: Some(self),
            ..HttpResponse::new()
        })
    }
}

impl IntoOutput<Http> for u16 {
    fn into_output(self) -> Answer<Http> {
        Ok(HttpResponse {
            status: self,
            ..HttpResponse::new()
        })
    }
}

impl IntoOutput<Http> for Vec<(String, String)> {
    fn into_output(self) -> Answer<Http> {
        Ok(HttpResponse {
            headers: self,
            ..HttpResponse::new()
        })
    }
}

impl IntoOutput<Http> for (u16, Body) {
    fn into_output(self) -> Answer<Http> {
        Ok(HttpResponse {
            status: self.0,
            body: Some(self.1),
            ..HttpResponse::new()
        })
    }
}

impl IntoOutput<Http> for Value {
    fn into_output(self) -> Answer<Http> {
        Ok(HttpResponse {
            body: Some(Body::json(self)),
            ..HttpResponse::new()
        })
    }
}

impl IntoOutput<Http> for String {
    fn into_output(self) -> Answer<Http> {
        Ok(HttpResponse {
            body: Some(Body::text(self)),
            ..HttpResponse::new()
        })
    }
}

impl IntoOutput<Http> for &'static str {
    fn into_output(self) -> Answer<Http> {
        Ok(HttpResponse {
            body: Some(Body::text(self)),
            ..HttpResponse::new()
        })
    }
}
