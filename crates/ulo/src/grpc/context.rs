use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::context::Metadata;
use crate::grpc::runtime::{RequestCarrier, RequestError};

use crate::context::shared::SharedState;
use crate::context::{CancellationToken, ExecutionContext, Extensions};

/// Per-request context for gRPC handlers.
///
/// gRPC payloads are method-typed protobuf messages and can't sit in a
/// non-generic struct, so what every enhancer can name without a type
/// parameter is held typed: the method path, the inbound metadata (ASCII
/// headers), and the optional peer address. The message itself rides erased,
/// in a slot a handler parameter takes once — `Payload<T>`, `Inbound<T>`, or
/// `ulo_grpc::GrpcRequest<T>` — which is how a `#[grpc_methods]` handler's
/// parameters are all extractors.
///
/// Guards, interceptors and error handlers receive this context as a
/// parameter. A service written against tonic's own trait and registered
/// through `add_service` takes it off the request instead — [`GrpcContext::of`],
/// or `Extensions::adopt(request.extensions())` for the bag alone.
#[derive(Clone)]
pub struct GrpcContext {
    inner: Arc<GrpcInner>,
}

struct GrpcInner {
    shared: SharedState,
    method: String,
    headers: HashMap<String, String>,
    peer: Option<SocketAddr>,
    /// Read from `grpc-timeout` at construction, so every reader sees one
    /// deadline rather than each recomputing from a clock that has moved.
    deadline: Option<Instant>,
    request: Mutex<RequestSlot>,
}

/// One request per execution, taken once: a stream has nothing to hand a
/// second reader, and a message follows the same rule so the two extract alike.
enum RequestSlot {
    Empty,
    Installed(Box<dyn RequestCarrier>),
    Taken,
}

impl GrpcContext {
    pub fn new(
        method: impl Into<String>,
        headers: HashMap<String, String>,
        peer: Option<SocketAddr>,
        metadata: Option<Arc<Metadata>>,
    ) -> Self {
        let deadline = headers
            .get("grpc-timeout")
            .and_then(|value| parse_grpc_timeout(value))
            .map(|budget| Instant::now() + budget);
        Self {
            inner: Arc::new(GrpcInner {
                shared: SharedState::new(metadata),
                method: method.into(),
                headers,
                peer,
                deadline,
                request: Mutex::new(RequestSlot::Empty),
            }),
        }
    }

    /// Hand the execution its request. `#[grpc_methods]` does this once, before
    /// the handler's parameters are extracted.
    #[doc(hidden)]
    pub fn install_request(&self, carrier: Box<dyn RequestCarrier>) {
        let mut slot = self.inner.request.lock().unwrap_or_else(|e| e.into_inner());
        *slot = RequestSlot::Installed(carrier);
    }

    /// A copy of the message the call carries, leaving it for the handler.
    ///
    /// For a guard or interceptor deciding on the message before the handler
    /// runs: the request is installed ahead of the guards, and a copy is what
    /// keeps the handler's `Payload<T>` whole. `T` is the method's request
    /// message; another type answers [`RequestError::Mismatch`] naming both,
    /// and a method whose caller streams answers [`RequestError::Streamed`],
    /// since a stream has one reader.
    ///
    /// ```ignore
    /// async fn can_activate(&self, ctx: &GrpcContext) -> bool {
    ///     ctx.message::<CreateOrderRequest>()
    ///         .map(|req| req.qty <= self.max_qty)
    ///         .unwrap_or(false)
    /// }
    /// ```
    pub fn message<T: 'static>(&self) -> Result<T, RequestError> {
        let slot = self.inner.request.lock().unwrap_or_else(|e| e.into_inner());
        let carrier = match &*slot {
            RequestSlot::Installed(carrier) => carrier,
            RequestSlot::Taken => return Err(RequestError::Taken),
            RequestSlot::Empty => return Err(RequestError::Missing),
        };
        let copy = carrier.clone_message().ok_or(RequestError::Streamed)?;
        copy.downcast::<T>()
            .map(|message| *message)
            .map_err(|_| RequestError::Mismatch {
                asked: std::any::type_name::<T>(),
                carried: carrier.carries(),
            })
    }

    /// Take the request, once. The first line of an extractor that reads the
    /// message; the built-in ones downcast what comes back.
    pub fn take_request(&self) -> Result<Box<dyn RequestCarrier>, RequestError> {
        let mut slot = self.inner.request.lock().unwrap_or_else(|e| e.into_inner());
        match std::mem::replace(&mut *slot, RequestSlot::Taken) {
            RequestSlot::Installed(carrier) => Ok(carrier),
            RequestSlot::Taken => Err(RequestError::Taken),
            RequestSlot::Empty => {
                *slot = RequestSlot::Empty;
                Err(RequestError::Missing)
            }
        }
    }

    /// The method path the call arrived on, as the caller dialled it:
    /// `package.Service/Method`.
    ///
    /// A guard or interceptor matching on it matches what a proto file, a log
    /// line and a client stub all spell the same way. On a pipeline driven
    /// without an adapter — a test calling the generated method directly — it
    /// falls back to the trait and method names as written, which carry no
    /// package.
    pub fn method(&self) -> &str {
        &self.inner.method
    }

    /// The wire fields that arrived with this call.
    ///
    /// gRPC's specification calls these metadata; `headers` is the one name this framework uses for all of them, leaving `metadata`
    /// to mean what `#[set_metadata]` declared.
    #[doc(alias = "metadata")]
    pub fn headers(&self) -> &HashMap<String, String> {
        &self.inner.headers
    }

    /// One wire field by key.
    #[doc(alias = "metadata")]
    pub fn header(&self, key: &str) -> Option<&str> {
        self.inner.headers.get(key).map(|s| s.as_str())
    }

    pub fn peer(&self) -> Option<SocketAddr> {
        self.inner.peer
    }

    /// The context riding a gRPC request, which is how a handler reaches one.
    ///
    /// `#[grpc_methods]` puts it there before the handler runs, so this answers
    /// `Some` for any service the framework dispatches. A service handed to
    /// tonic directly through `GrpcAdapter::add_service` has no execution
    /// behind it and answers `None`.
    ///
    /// ```ignore
    /// async fn watch(&self, request: Request<WatchRequest>)
    ///     -> Result<Response<Self::WatchStream>, Status>
    /// {
    ///     let ctx = GrpcContext::of(request.extensions()).expect("dispatched by ulo");
    ///     let cancelled = ctx.cancellation().clone();
    ///     // …stop feeding the reply once `cancelled.cancelled()` resolves
    /// }
    /// ```
    pub fn of(carrier: &http::Extensions) -> Option<Self> {
        carrier.get::<Self>().cloned()
    }
}

impl ExecutionContext for GrpcContext {
    fn metadata(&self) -> Option<&Metadata> {
        self.inner.shared.metadata.as_deref()
    }

    fn extensions(&self) -> &Extensions {
        &self.inner.shared.extensions
    }

    fn cache(&self) -> &crate::di::ExecutionCache {
        &self.inner.shared.cache
    }

    fn cancellation(&self) -> &CancellationToken {
        &self.inner.shared.cancellation
    }

    fn deadline(&self) -> Option<Instant> {
        self.inner.deadline
    }
}

/// Parse the `grpc-timeout` header: up to eight digits followed by a unit.
///
/// Defined by the [gRPC HTTP/2 spec][spec]. A value this cannot read is treated
/// as absent — the call is answered rather than refused over a header the caller
/// may not know it sent, and tonic refuses the malformed ones it enforces
/// itself.
///
/// [spec]: https://github.com/grpc/grpc/blob/master/doc/PROTOCOL-HTTP2.md
fn parse_grpc_timeout(value: &str) -> Option<Duration> {
    let (digits, unit) = value.split_at(value.len().checked_sub(1)?);
    if digits.is_empty() || digits.len() > 8 {
        return None;
    }
    let amount: u64 = digits.parse().ok()?;
    match unit {
        "H" => Some(Duration::from_secs(amount * 60 * 60)),
        "M" => Some(Duration::from_secs(amount * 60)),
        "S" => Some(Duration::from_secs(amount)),
        "m" => Some(Duration::from_millis(amount)),
        "u" => Some(Duration::from_micros(amount)),
        "n" => Some(Duration::from_nanos(amount)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_unit_the_spec_defines() {
        assert_eq!(parse_grpc_timeout("5S"), Some(Duration::from_secs(5)));
        assert_eq!(parse_grpc_timeout("2H"), Some(Duration::from_secs(7200)));
        assert_eq!(parse_grpc_timeout("3M"), Some(Duration::from_secs(180)));
        assert_eq!(parse_grpc_timeout("250m"), Some(Duration::from_millis(250)));
        assert_eq!(parse_grpc_timeout("40u"), Some(Duration::from_micros(40)));
        assert_eq!(parse_grpc_timeout("7n"), Some(Duration::from_nanos(7)));
    }

    #[test]
    fn a_value_the_spec_does_not_define_reads_as_absent() {
        assert_eq!(parse_grpc_timeout(""), None);
        assert_eq!(parse_grpc_timeout("S"), None, "no digits");
        assert_eq!(parse_grpc_timeout("5"), None, "no unit");
        assert_eq!(parse_grpc_timeout("5X"), None, "unknown unit");
        assert_eq!(parse_grpc_timeout("-1S"), None, "not a count");
        assert_eq!(parse_grpc_timeout("123456789S"), None, "over eight digits");
    }

    #[test]
    fn a_context_carries_the_deadline_its_headers_named() {
        let mut headers = HashMap::new();
        headers.insert("grpc-timeout".to_string(), "5S".to_string());
        let ctx = GrpcContext::new("pkg.Svc/Method", headers, None, None);

        let remaining = ctx
            .deadline()
            .expect("a deadline")
            .saturating_duration_since(Instant::now());
        assert!(
            remaining > Duration::from_secs(4) && remaining <= Duration::from_secs(5),
            "remaining: {remaining:?}"
        );
    }

    #[test]
    fn a_context_without_the_header_carries_none() {
        let ctx = GrpcContext::new("pkg.Svc/Method", HashMap::new(), None, None);
        assert!(ctx.deadline().is_none());
    }

    /// Stands in for the carriers ulo-grpc builds around `tonic::Request`.
    struct Carrying(u32);

    impl RequestCarrier for Carrying {
        fn take_message(self: Box<Self>) -> Box<dyn std::any::Any + Send> {
            Box::new(self.0)
        }
        fn into_any(self: Box<Self>) -> Box<dyn std::any::Any + Send> {
            self
        }
        fn clone_message(&self) -> Option<Box<dyn std::any::Any + Send>> {
            Some(Box::new(self.0))
        }
        fn carries(&self) -> &'static str {
            "u32"
        }
    }

    #[test]
    fn a_copy_leaves_the_request_for_the_handler() {
        let ctx = GrpcContext::new("pkg.Svc/Method", HashMap::new(), None, None);
        assert_eq!(ctx.message::<u32>().err(), Some(RequestError::Missing));

        ctx.install_request(Box::new(Carrying(7)));
        assert_eq!(ctx.message::<u32>(), Ok(7));
        assert_eq!(ctx.message::<u32>(), Ok(7), "a copy is not a take");
        assert_eq!(
            ctx.message::<String>().err(),
            Some(RequestError::Mismatch {
                asked: std::any::type_name::<String>(),
                carried: "u32",
            })
        );

        let carrier = ctx
            .take_request()
            .expect("still installed after two copies");
        assert_eq!(
            carrier.take_message().downcast::<u32>().ok(),
            Some(Box::new(7))
        );
        assert_eq!(ctx.message::<u32>().err(), Some(RequestError::Taken));
    }

    #[test]
    fn the_request_is_taken_once_and_each_failure_says_why() {
        let ctx = GrpcContext::new("pkg.Svc/Method", HashMap::new(), None, None);
        assert_eq!(ctx.take_request().err(), Some(RequestError::Missing));

        ctx.install_request(Box::new(Carrying(7)));
        let carrier = ctx.take_request().expect("installed");
        assert_eq!(
            carrier.take_message().downcast::<u32>().ok(),
            Some(Box::new(7))
        );

        assert_eq!(ctx.take_request().err(), Some(RequestError::Taken));
    }

    #[tokio::test]
    async fn a_message_extractor_names_both_types_when_the_call_carries_another() {
        use crate::extract::{FromContext, Payload};
        let ctx = GrpcContext::new("pkg.Svc/Method", HashMap::new(), None, None);
        ctx.install_request(Box::new(Carrying(7)));

        let err = <Payload<String> as FromContext<GrpcContext>>::extract(&ctx)
            .await
            .expect_err("a String is not what the call carries");
        assert_eq!(
            err,
            RequestError::Mismatch {
                asked: std::any::type_name::<String>(),
                carried: "u32",
            }
        );
        assert!(err.to_string().contains("u32"), "{err}");
    }
}
