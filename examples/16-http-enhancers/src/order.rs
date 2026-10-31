//! When guards are built and in what order error handlers are asked.
//!
//! A guard is built only once the one before it has admitted, so a refusal leaves the rest
//! unbuilt. Error handlers are asked method tier first, then controller, then global, and within a
//! tier the last one declared first; the first to answer claims the error.

use ulo::{BoxError, Dep, ErrorHandler, Guard, injectable, routes};
use ulo_http::{CallError, ErrorKind, Http, HttpBody, HttpCx, Path, Response, StatusCode};

use crate::Trace;

/// Records when it is built and when it runs, and admits or refuses as it was told.
pub struct Gate {
    label: &'static str,
    admit: bool,
    trace: Dep<Trace>,
}

impl Gate {
    fn new(label: &'static str, admit: bool, trace: Dep<Trace>) -> Self {
        trace.push(format!("built {label}"));
        Gate { label, admit, trace }
    }
}

impl Guard<Http> for Gate {
    async fn can_activate(&self, _cx: &HttpCx) -> Result<bool, BoxError> {
        self.trace.push(format!("ran {}", self.label));
        Ok(self.admit)
    }
}

#[derive(Debug, ulo_http::Classify)]
#[classify(conflict)]
pub struct OutOfStock(pub u32);

impl std::fmt::Display for OutOfStock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "sku {} is out of stock", self.0)
    }
}

impl std::error::Error for OutOfStock {}

/// Records the error it saw and hands it on: observation without shaping.
#[injectable]
pub struct Telemetry {
    trace: Dep<Trace>,
}

impl ErrorHandler<Http> for Telemetry {
    async fn handle(&self, err: BoxError, _cx: &HttpCx) -> Result<Response, BoxError> {
        self.trace.push(format!("telemetry: {err}"));
        Err(err)
    }
}

/// Records its tier and hands the error on.
macro_rules! passes {
    ($name:ident, $label:literal) => {
        #[injectable]
        pub struct $name {
            trace: Dep<Trace>,
        }

        impl ErrorHandler<Http> for $name {
            async fn handle(&self, err: BoxError, _cx: &HttpCx) -> Result<Response, BoxError> {
                self.trace.push($label);
                Err(err)
            }
        }
    };
}

passes!(Method, "method");
passes!(Controller, "controller");

/// Answers 200 `claimed by method`.
#[injectable]
pub struct Claimer {
    trace: Dep<Trace>,
}

impl ErrorHandler<Http> for Claimer {
    async fn handle(&self, _err: BoxError, _cx: &HttpCx) -> Result<Response, BoxError> {
        self.trace.push("claimer");
        Ok(Response::new(HttpBody::from("claimed by method")))
    }
}

/// The global handler: answers an `OutOfStock` as 409 and leaves every other error alone.
#[injectable]
pub struct Shape {
    trace: Dep<Trace>,
}

impl ErrorHandler<Http> for Shape {
    async fn handle(&self, err: BoxError, _cx: &HttpCx) -> Result<Response, BoxError> {
        let call = CallError::from_boxed(err);
        let Some(OutOfStock(sku)) = call.source_as::<OutOfStock>() else { return Err(Box::new(call)) };
        self.trace.push("global");
        let mut reply = Response::new(HttpBody::from(format!("sku {sku} is sold out")));
        *reply.status_mut() = StatusCode::CONFLICT;
        debug_assert_eq!(call.kind(), ErrorKind::Conflict);
        Ok(reply)
    }
}

#[injectable]
pub struct OrderedController {
    trace: Dep<Trace>,
}

#[routes]
#[error_handlers(Controller)]
impl OrderedController {
    #[ulo_http::get("/lazy/closed")]
    #[guards(with = |trace: Dep<Trace>| Gate::new("first", false, trace), with = |trace: Dep<Trace>| Gate::new("second", true, trace))]
    fn closed(&self) -> &'static str {
        self.trace.push("handler");
        "unreachable"
    }

    #[ulo_http::get("/lazy/open")]
    #[guards(with = |trace: Dep<Trace>| Gate::new("first", true, trace), with = |trace: Dep<Trace>| Gate::new("second", true, trace))]
    fn open(&self) -> &'static str {
        self.trace.push("handler");
        "open"
    }

    /// `Method` is declared last, so it is asked before `Telemetry`.
    #[ulo_http::get("/stock/{sku}")]
    #[error_handlers(Telemetry, Method)]
    fn stock(&self, Path(sku): Path<u32>) -> Result<&'static str, OutOfStock> {
        Err(OutOfStock(sku))
    }

    #[ulo_http::get("/stock-claimed/{sku}")]
    #[error_handlers(Telemetry, Claimer)]
    fn stock_claimed(&self, Path(sku): Path<u32>) -> Result<&'static str, OutOfStock> {
        Err(OutOfStock(sku))
    }
}
