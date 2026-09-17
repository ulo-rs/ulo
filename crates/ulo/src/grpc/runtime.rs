//! Per-call helpers invoked from `#[grpc_methods]`-generated code.
//!
//! Lives here so the chain logic is tonic-free, unit-testable, and shared
//! across every gRPC service the macro emits. The macro generates a thin
//! per-method shim that builds a [`GrpcContext`], hands this module the user's
//! handler as a delegate, and maps whatever comes back to tonic's types.

use crate::dispatch::transport::Grpc;
use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;

use async_trait::async_trait;

use crate::enhancer::{Interceptor, InterceptorNext};
use crate::errors::{GuardRejection, PipelineSegment};
use crate::grpc::GrpcContext;
use crate::grpc::GrpcHandlerResult;
use crate::grpc::GrpcStatus;
use crate::grpc::ResolvedGrpcEnhancers;
use crate::panic_recovery::catch_async;

/// Run guards, then the interceptor chain, then the error chain over whatever failed.
///
/// `delegate` is the user's handler, packaged by the macro as a closure answering this transport's
/// [`GrpcHandlerResult`]: `Ok(())` once the typed reply is in the macro's side-channel — the user's
/// `Result<Response<_>, Status>` is method-specific and cannot fit a generic chain-runner signature
/// — and `Err` for anything that failed below the chain.
///
/// Every way a call can fail leaves as `Err(GrpcStatus)` carrying its own cause: a refusal carries
/// its [`GuardRejection`], a panic anywhere below carries its `PanicRecovered`, a handler's failure
/// carries the domain error it raised. So the chain runs here, once, over all of them, rather than
/// at each level that can produce one.
pub async fn run_grpc_pipeline<D, Fut>(
    ctx: &GrpcContext,
    enhancers: &ResolvedGrpcEnhancers,
    method: &str,
    delegate: D,
) -> GrpcHandlerResult
where
    D: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = GrpcHandlerResult> + Send + 'static,
{
    let answer = match run_grpc_guards(ctx, enhancers, method).await {
        Ok(()) => {
            let mut all_interceptors = enhancers.interceptors.clone();
            if let Some(per_method) = enhancers.handler_interceptors.get(method) {
                all_interceptors.extend_from_slice(per_method);
            }
            let interceptors =
                crate::enhancer::pipeline::interceptors_for::<Grpc>(&all_interceptors, ctx).await;
            execute_with_interceptors(ctx, &interceptors, delegate).await
        }
        Err(refused) => Err(refused),
    };

    let Err(status) = answer else {
        return answer;
    };

    // The chain is offered the cause where the status carries one, so `#[catch(MyError)]` matches
    // what the handler raised and `#[catch(GuardRejection)]` the refusal, rather than the status
    // each of them flattened into. Bound to a local: the borrow has to end before the `Err` below
    // takes the status back.
    let mut handlers = enhancers.error_handlers.clone();
    if let Some(per_method) = enhancers.handler_error_handlers.get(method) {
        handlers.extend_from_slice(per_method);
    }
    let claimed = {
        let observed: &(dyn std::error::Error + Send + Sync + 'static) = match status.source() {
            Some(cause) => cause,
            None => &status,
        };
        crate::enhancer::pipeline::claim::<Grpc>(&handlers, observed, ctx).await
    };
    // A claim answers what an interceptor answers: `Ok` recovers the call with a reply of its own,
    // `Err` reshapes the failure. Unclaimed, the status the call failed with is the answer.
    claimed.unwrap_or(Err(status))
}

/// The guards this call runs, in declaration order.
///
/// A refusal and a panic both leave as an `Err` carrying the event, which is what the chain above
/// is offered. Unclaimed, the status each was built with is what reaches the caller.
async fn run_grpc_guards(
    ctx: &GrpcContext,
    enhancers: &ResolvedGrpcEnhancers,
    method: &str,
) -> Result<(), GrpcStatus> {
    let mut all_guards = enhancers.guards.clone();
    if let Some(per_method) = enhancers.handler_guards.get(method) {
        all_guards.extend_from_slice(per_method);
    }

    let guards = crate::enhancer::pipeline::guards_for::<Grpc>(&all_guards, ctx).await;
    for (index, guard) in guards.iter().enumerate() {
        // A panicking guard is a bug, not a verdict: it carries `PanicRecovered` rather than a
        // rejection, so an unclaimed one renders `Internal` rather than telling the caller its
        // credentials were refused.
        let activated = match catch_async(PipelineSegment::Guard, guard.can_activate(ctx)).await {
            Ok(b) => b,
            Err(event) => {
                tracing::debug!(guard_index = index, panic = %event.message, "guard panicked");
                return Err(GrpcStatus::new(
                    crate::grpc::GrpcCode::Internal,
                    format!("guard {} panicked: {}", index, event.message),
                )
                .caused_by(event));
            }
        };
        if !activated {
            return Err(
                GrpcStatus::permission_denied(format!("guard {} rejected request", index))
                    .caused_by(GuardRejection::new(index)),
            );
        }
    }
    Ok(())
}

/// Linked chain of interceptors wrapping a final delegate. Mirrors
/// `RpcControllerWrapper::execute_with_interceptors_impl` — each `Box<Self>`
/// move on `InterceptorNext::run` enforces the once-only invocation
/// contract.
async fn execute_with_interceptors<D, Fut>(
    ctx: &GrpcContext,
    interceptors: &[Arc<dyn Interceptor<GrpcContext, GrpcHandlerResult>>],
    delegate: D,
) -> GrpcHandlerResult
where
    D: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = GrpcHandlerResult> + Send + 'static,
{
    if interceptors.is_empty() {
        return delegate().await;
    }

    let next = build_next(&interceptors[1..], delegate);
    match catch_async(
        PipelineSegment::Middleware,
        interceptors[0].intercept(ctx, next),
    )
    .await
    {
        Ok(answer) => answer,
        Err(event) => Err(interceptor_panicked(event)),
    }
}

fn build_next<D, Fut>(
    rest: &[Arc<dyn Interceptor<GrpcContext, GrpcHandlerResult>>],
    delegate: D,
) -> Box<dyn InterceptorNext<GrpcContext, GrpcHandlerResult>>
where
    D: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = GrpcHandlerResult> + Send + 'static,
{
    if rest.is_empty() {
        Box::new(LeafNext { delegate })
    } else {
        Box::new(LinkNext {
            head: rest[0].clone(),
            rest: rest[1..].to_vec(),
            delegate,
        })
    }
}

/// The status a panicking interceptor answers with, carrying the event the chain above is offered.
fn interceptor_panicked(event: crate::errors::PanicRecovered) -> GrpcStatus {
    GrpcStatus::new(
        crate::grpc::GrpcCode::Internal,
        format!("interceptor panicked: {}", event.message),
    )
    .caused_by(event)
}

/// Innermost link: invokes the user delegate.
struct LeafNext<D> {
    delegate: D,
}

#[async_trait]
impl<D, Fut> InterceptorNext<GrpcContext, GrpcHandlerResult> for LeafNext<D>
where
    D: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = GrpcHandlerResult> + Send + 'static,
{
    async fn run(self: Box<Self>, _ctx: &GrpcContext) -> GrpcHandlerResult {
        (self.delegate)().await
    }
}

/// Outer link: hands off to the next interceptor in line.
struct LinkNext<D> {
    head: Arc<dyn Interceptor<GrpcContext, GrpcHandlerResult>>,
    rest: Vec<Arc<dyn Interceptor<GrpcContext, GrpcHandlerResult>>>,
    delegate: D,
}

#[async_trait]
impl<D, Fut> InterceptorNext<GrpcContext, GrpcHandlerResult> for LinkNext<D>
where
    D: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = GrpcHandlerResult> + Send + 'static,
{
    async fn run(self: Box<Self>, ctx: &GrpcContext) -> GrpcHandlerResult {
        let this = *self;
        let next = build_next(&this.rest, this.delegate);
        match catch_async(PipelineSegment::Middleware, this.head.intercept(ctx, next)).await {
            Ok(answer) => answer,
            Err(event) => Err(interceptor_panicked(event)),
        }
    }
}

/// The reply a gRPC call answers with: its headers reachable, its message erased.
///
/// A reply's type is the method's — tonic's trait names it, and for a streaming method it is an
/// associated type the user's impl defines — while one guard, interceptor and error-handler list
/// serves every method of a service. So the message cannot be in the signature those share, and it
/// travels as the value it is with its name beside it.
///
/// Its headers are not the method's. [`header`](Self::header) and [`set_header`](Self::set_header)
/// name no reply type, so an interceptor stamping every reply of a service is written once.
/// Reaching the message names it, through [`downcast`](Self::downcast), and an enhancer doing that
/// is answering for one method.
///
/// Built where `tonic::Response<T>` is nameable, which is the wire crate: the generated wrapper
/// wraps the reply on the way in and downcasts it on the way out.
pub struct GrpcReply(Box<dyn ReplyEnvelope>);

impl GrpcReply {
    /// Hold a reply. The argument is the wire crate's carrier around the
    /// `tonic::Response<_>` the method answers with.
    pub fn new(envelope: impl ReplyEnvelope) -> Self {
        Self(Box::new(envelope))
    }

    /// A header on the reply, where it is set and its value is ASCII.
    ///
    /// The method's reply type does not appear, so an enhancer serving every method of a service
    /// reads one without knowing which method answered.
    pub fn header(&self, key: &str) -> Option<&str> {
        self.0.header(key)
    }

    /// Set a header on the reply, replacing any value already under the key.
    ///
    /// The counterpart to [`header`](Self::header), and the same reason: an interceptor stamping
    /// every reply of a service writes this once.
    pub fn set_header(&mut self, key: &str, value: &str) -> Result<(), InvalidHeader> {
        self.0.set_header(key, value)
    }

    /// Take the reply, or hand it back untouched where it carries something else.
    ///
    /// The type is checked through a borrow before the carrier is consumed, so a reply handed back
    /// still has its envelope. An enhancer trying one reply type and then another keeps whatever
    /// headers are on it across the first attempt.
    pub fn downcast<T: Send + 'static>(self) -> Result<T, Self> {
        if !self.0.as_any().is::<T>() {
            return Err(self);
        }
        match self.0.into_any().downcast::<T>() {
            Ok(value) => Ok(*value),
            // `is::<T>` answered for this value one line above.
            Err(_) => unreachable!("a reply that is `T` downcasts to `T`"),
        }
    }

    /// Read the reply where it is what the reader expects, leaving it in place.
    pub fn downcast_ref<T: Send + 'static>(&self) -> Option<&T> {
        self.0.as_any().downcast_ref::<T>()
    }

    /// Read the reply to change it in place.
    pub fn downcast_mut<T: Send + 'static>(&mut self) -> Option<&mut T> {
        self.0.as_any_mut().downcast_mut::<T>()
    }

    /// What it carries, named for the diagnostic when something asks for another type.
    pub fn carries(&self) -> &'static str {
        self.0.carries()
    }
}

impl std::fmt::Debug for GrpcReply {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GrpcReply")
            .field("carries", &self.0.carries())
            .finish()
    }
}

/// What a gRPC reply carries beside its message, and what an enhancer may do to it.
///
/// `ulo` names no tonic type, so the envelope reaches core as a trait the wire crate implements —
/// the road [`RequestCarrier`] takes for the request. `ulo-grpc` implements this on a newtype over
/// `tonic::Response<T>`, which is what the orphan rule leaves available.
///
/// The header methods name no reply type, which is what lets one enhancer list serve every method
/// of a service. Reaching the message itself still names it, through [`GrpcReply::downcast`].
pub trait ReplyEnvelope: Send + 'static {
    /// One header of the reply, where it is set and its value is ASCII.
    fn header(&self, key: &str) -> Option<&str>;

    /// Set a header, replacing any value already under the key.
    fn set_header(&mut self, key: &str, value: &str) -> Result<(), InvalidHeader>;

    /// The reply whole, for a reader naming the method's own type.
    fn into_any(self: Box<Self>) -> Box<dyn std::any::Any + Send>;

    /// The reply, borrowed.
    fn as_any(&self) -> &(dyn std::any::Any + Send);

    /// The reply, borrowed to change in place.
    fn as_any_mut(&mut self) -> &mut (dyn std::any::Any + Send);

    /// What it carries, named for the diagnostic when something asks for another type.
    fn carries(&self) -> &'static str;
}

/// A header key or value the wire cannot carry.
///
/// gRPC metadata keys are lowercase ASCII tokens and a non-`-bin` value is ASCII. A key or value
/// outside that is refused rather than dropped, so a stamping interceptor learns its header did
/// not go out.
///
/// Nothing lifts this into a [`GrpcStatus`], so `?` does not reach for one. Whether a header that
/// did not go out should fail the call depends on where its key came from, and only the enhancer
/// knows: a literal is the author's to get right, which `expect` states, while a key read from
/// configuration or echoed off the request can be malformed on one call out of many, and failing
/// that call serves the caller worse than dropping the stamp.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidHeader {
    pub key: String,
}

impl std::fmt::Display for InvalidHeader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "`{}` is not a header the wire can carry", self.key)
    }
}

impl std::error::Error for InvalidHeader {}

/// Carries a domain error through a `tonic::Status`'s source slot.
///
/// That slot is typed `dyn std::error::Error`, which drops the `Send + Sync`
/// the error chain needs. Wrapping the error in a concrete type is what lets a
/// downcast on the way out recover the bound.
#[derive(Debug)]
pub struct GrpcFailure(Arc<dyn crate::errors::Error>);

impl GrpcFailure {
    pub fn new(error: Arc<dyn crate::errors::Error>) -> Self {
        Self(error)
    }

    /// The error a status carries, read off the source slot.
    ///
    /// The way back for a caller holding a `tonic::Status` ulo produced — a tower layer, or a
    /// service of its own wrapping one of ulo's. Inside ulo's dispatch a failure never flattens:
    /// it reaches the chain as the `GrpcStatus` it was raised with, error and all.
    pub fn recover(
        source: Option<&(dyn std::error::Error + 'static)>,
    ) -> Option<Arc<dyn crate::errors::Error>> {
        source?.downcast_ref::<Self>().map(|f| f.0.clone())
    }
}

impl std::fmt::Display for GrpcFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl std::error::Error for GrpcFailure {}

/// Wrap a future in `AssertUnwindSafe(...).catch_unwind()` and surface
/// the panic payload as a [`PanicRecovered`](crate::errors::PanicRecovered)
/// event scoped to the
/// `HandlerBody` segment. Used by the macro around the user delegation
/// inside a `#[grpc_methods]` proto method.
///
/// Thin wrapper around the shared `catch_async` helper so the
/// macro can keep a stable, transport-specific entry point even as the
/// shared helper evolves.
pub async fn catch_handler_panic<Fut, T>(fut: Fut) -> Result<T, crate::errors::PanicRecovered>
where
    Fut: Future<Output = T>,
{
    crate::panic_recovery::catch_async(crate::errors::PipelineSegment::HandlerBody, fut).await
}

/// Empty bundle helper — used by the gRPC adapter when a service hasn't
/// been resolved through the framework (e.g. wired directly via
/// `add_service` on the adapter), and by tests.
#[doc(hidden)]
pub fn empty_enhancers() -> Arc<ResolvedGrpcEnhancers> {
    Arc::new(ResolvedGrpcEnhancers {
        guards: Vec::new(),
        handler_guards: HashMap::new(),
        interceptors: Vec::new(),
        handler_interceptors: HashMap::new(),
        error_handlers: Vec::new(),
        handler_error_handlers: HashMap::new(),
    })
}

/// Delegates to a streaming reply while owning the execution's context — cache,
/// extensions and cancellation token stay alive until the last item.
///
/// The `#[grpc_methods]` wrapper declares this as its associated stream type,
/// so it is the reply tonic serves. `Pin<Box<_>>` rather than a pin projection:
/// the inner stream is the user's associated type and carries no `Unpin` bound.
pub struct ScopedGrpcStream<S> {
    inner: std::pin::Pin<Box<S>>,
    context: GrpcContext,
    /// Set once the inner stream answers `None`. An item carrying a `Status`
    /// does not set it: tonic ends the call there and drops this un-drained, so
    /// the producer behind an abnormal end hears the token too.
    drained: bool,
}

impl<S> ScopedGrpcStream<S> {
    pub fn new(inner: S, context: GrpcContext) -> Self {
        Self {
            inner: Box::pin(inner),
            context,
            drained: false,
        }
    }
}

impl<S: futures::Stream> futures::Stream for ScopedGrpcStream<S> {
    type Item = S::Item;

    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        let this = self.get_mut();
        let polled = this.inner.as_mut().poll_next(cx);
        if matches!(polled, std::task::Poll::Ready(None)) {
            this.drained = true;
        }
        polled
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

/// A stream dropped with items still to come is the caller having gone — a reset
/// stream, a dead connection, or the drain deadline dropping the server. The
/// handler returned when it had a stream, so whatever feeds that stream is not
/// inside a future tonic drops.
impl<S> Drop for ScopedGrpcStream<S> {
    fn drop(&mut self) {
        if !self.drained {
            use crate::context::ExecutionContext as _;
            self.context.cancellation().cancel();
        }
    }
}

/// Carries a reply from the type the user's method produced to the type the
/// generated wrapper's signature declares.
///
/// The wrapper cannot know per method whether it is re-typing a stream or
/// passing a message through, and the answer is decided by the target type
/// alone: a message resolves to the identity impl, a stream whose associated
/// type the wrapper rewrote resolves to the wrapping one. The two never
/// overlap, since that would need `S == ScopedGrpcStream<S>`.
#[doc(hidden)]
pub trait IntoScoped<Out> {
    fn into_scoped(self, context: GrpcContext) -> Out;
}

impl<T> IntoScoped<T> for T {
    fn into_scoped(self, _context: GrpcContext) -> T {
        self
    }
}

impl<S: futures::Stream> IntoScoped<ScopedGrpcStream<S>> for S {
    fn into_scoped(self, context: GrpcContext) -> ScopedGrpcStream<S> {
        ScopedGrpcStream::new(self, context)
    }
}

/// The request a gRPC call arrived with, held by the execution until a
/// handler parameter takes it.
///
/// `#[grpc_methods]` installs one before the handler's parameters are
/// extracted, through the `MethodShape` ulo-build wrote for the method. A
/// carrier is built where `tonic::Request` is nameable and reaches this crate
/// erased, which is what lets `Payload<T>` and `Inbound<T>` take a message
/// without core naming the wire crate.
pub trait RequestCarrier: Send + 'static {
    /// The message in the shape a handler reads it: `T` for one message,
    /// `Inbound<T>` for a stream the caller sends.
    fn take_message(self: Box<Self>) -> Box<dyn std::any::Any + Send>;

    /// The carrier whole, for an extractor that wants the wire's own view.
    fn into_any(self: Box<Self>) -> Box<dyn std::any::Any + Send>;

    /// A copy of the message, for a reader that leaves it in place — a guard
    /// or interceptor deciding on it before the handler takes it. `None`
    /// where the caller streams: a stream has one reader.
    fn clone_message(&self) -> Option<Box<dyn std::any::Any + Send>>;

    /// What the call carries, named for the diagnostic when a handler asks
    /// for something else.
    fn carries(&self) -> &'static str;
}

/// Why a handler parameter could not take the request.
///
/// Each is a fault in the handler or the dispatch rather than in the call, so
/// the generated method answers `Internal` with the message.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum RequestError {
    /// A parameter before this one took it. The macro rejects two takers at
    /// compile time; this is what an extractor written around that sees.
    Taken,
    /// Nothing was installed: the method was reached outside ulo's dispatch.
    Missing,
    /// The handler asked for one type and the call carries another.
    Mismatch {
        asked: &'static str,
        carried: &'static str,
    },
    /// A copy was asked for and the caller streams. A stream has one reader:
    /// the handler, as `Inbound<T>`.
    Streamed,
}

impl std::fmt::Display for RequestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RequestError::Taken => {
                f.write_str("the request was already taken by an earlier parameter")
            }
            RequestError::Missing => f.write_str(
                "no request was installed on this execution — the method was reached outside \
                 ulo's dispatch",
            ),
            RequestError::Mismatch { asked, carried } => {
                write!(
                    f,
                    "the handler asked for `{asked}` but the call carries `{carried}`"
                )
            }
            RequestError::Streamed => f.write_str(
                "the caller streams, and a stream cannot be copied — the handler reads it as \
                 `Inbound<T>`",
            ),
        }
    }
}

impl std::error::Error for RequestError {}
