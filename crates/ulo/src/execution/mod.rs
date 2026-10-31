//! Executions: one call, or one standalone job (§3.8, §6.3).
//!
//! The transport holds the [`Execution`] for a call and `execute` holds it for a standalone one;
//! everything else holds an [`ExecutionRef`], directly or inside a transport's `Cx`. Three
//! events are distinct:
//! - **Draining** is the notice: it resolves as the drain begins, after the
//!   before-shutdown stage and a moment before the servers stop accepting. No execution opens
//!   from then on, so a request arriving in that moment is refused as the drain refuses one (503
//!   with `Connection: close` on HTTP), not served.
//! - **Cancellation** fires while handles are alive: for a call when the client disconnects or
//!   its deadline passes, for a standalone execution at its deadline, and for either at the end
//!   of the drain. Completion of `execute` does not fire it. Each fires with a [`CancelReason`]
//!   that a transport reads back to pick its rendering (transports DESIGN §2.6, X5).
//! - **End of execution** is when the [`Execution`] drops: the per-execution cache is released
//!   and execution-scoped instances drop then, whatever [`ExecutionRef`]s remain, so an instance
//!   holding one is no cycle. A transport holds the `Execution` until the reply is written, a
//!   streaming body included, so a streaming reply keeps its instances exactly as long as it
//!   runs. An `ExecutionRef` kept past the end still answers what the execution recorded, and
//!   its `get` answers `LookupError::ExecutionEnded`.

pub(crate) mod cache;
pub(crate) mod extensions;
pub(crate) mod notify;
pub(crate) mod trace;

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::Instant;

use crate::app::shared::{AppShared, LiveSlot};
use crate::dependency::Dep;
use crate::error::{Closed, LookupError};
use crate::key::{BindingKind, Key};
use crate::execution::cache::OnceCells;
use crate::execution::extensions::{Extensions, Seeded};
use crate::execution::notify::{Cancelled, Draining, Notify};
use crate::execution::trace::{TraceContext, TraceMode};
use crate::graph::ModuleId;
use crate::module::handle::ModuleRef;
use crate::resolver::{Purpose, Resolver};
use crate::transport::handler::HandlerInfo;
use crate::transport::server::DrainToken;

/// One execution, as its holder owns it. Not `Clone`: anything else takes a
/// [`handle`](Self::handle). Dropping it ends the execution (§3.8): the execution-scoped
/// instances drop, the drain stops waiting for it, and every handle's `get` answers
/// `LookupError::ExecutionEnded`.
///
/// `Execution: Send + Sync`, and so is the `Resolver` read through it, which is what lets every
/// read's future be `Send`.
pub struct Execution {
    pub(crate) shared: Arc<ExecShared>,
    /// Declared after `shared` and dropped after the cache is released, so the drain sees the end
    /// once the execution-scoped instances are gone.
    pub(crate) _live: LiveSlot,
}

/// A cheap-clone handle to an execution: the same methods as [`Execution`] except `seed` and
/// `route_to`, so an input a guard or a subtask seeded could never be one the wiring pass did not
/// see, and only the holder decides the module.
///
/// A handle does not keep the execution open. Kept past the end, it answers what the execution
/// recorded, its deadline, cancellation, extensions and trace among them, and its `get` answers
/// `LookupError::ExecutionEnded` rather than building an instance for an execution that is over.
#[derive(Clone)]
pub struct ExecutionRef {
    pub(crate) shared: Arc<ExecShared>,
}

/// Options for a new execution. Each setter keeps the others, so they compose in any order.
#[derive(Clone, Debug, Default)]
pub struct ExecOptions {
    pub(crate) deadline: Option<Instant>,
    pub(crate) trace: Option<TraceContext>,
    pub(crate) span: Option<tracing::Span>,
}

impl ExecOptions {
    pub fn new() -> Self {
        ExecOptions::default()
    }

    /// Cancellation fires when `at` passes, read on the app's `Timer`. The core stores the
    /// deadline; for a call the transport's runtime enforces it.
    pub fn deadline(self, at: Instant) -> Self {
        ExecOptions { deadline: Some(at), ..self }
    }

    /// The execution's inbound trace context, marked `Received` whatever it carried: a transport
    /// passes the context it extracted from the call, a standalone `execute` one it was handed.
    /// `Execution::trace()` answers it until a `TraceSource`'s answer replaces it. Dropped under
    /// `TraceMode::Off`.
    pub fn trace(self, ctx: TraceContext) -> Self {
        ExecOptions { trace: Some(ctx.received()), ..self }
    }

    /// A span its caller created and has not entered, for a standalone `execute` to trace: with a
    /// `TraceSource` bound, `execute` calls `TraceSource::start` with it and the inbound context,
    /// records the answer through `Execution::set_trace` and runs the closure inside it. Without
    /// a source, or under `TraceMode::Off`, the span is entered and nothing else happens.
    /// `Execution::open` reads no span: a transport traces a call through
    /// `ulo_transport::span::call`.
    pub fn span(self, span: tracing::Span) -> Self {
        ExecOptions { span: Some(span), ..self }
    }
}

/// Why an execution was cancelled (transports DESIGN §2.6, X5). Written once, beside the
/// cancellation signal: the first `cancel_with` decides it, and a cancelled execution always has
/// one.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CancelReason {
    /// The backend observed the peer close the connection or reset the stream: a dropped response
    /// body, a closed connection, an h2 `RST_STREAM`. The server dropping an unread request body
    /// is not one.
    Disconnected,
    /// A WebSocket or RPC `cancel` frame, or a gRPC `RST_STREAM(CANCEL)`.
    ClientCancelled,
    /// A passed `grpc-timeout`, RPC `deadline-ms` header, configured route timeout, or a
    /// standalone execution's deadline. A transport renders `Timeout` for it.
    Deadline,
    /// The core's end of the drain.
    Drain,
    /// `cancel()` by whoever holds the execution.
    Explicit,
}

/// How a reply stream ended, as `Execution::on_stream_end` callbacks receive it.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamOutcome {
    /// The stream returned `None` and the transport finished writing it.
    Completed,
    /// Dropped before that, with the execution's cancellation reason if it was cancelled. `None`
    /// when the stream was cut off without a cancellation: ended on an error item, or dropped by
    /// a transport that recorded no reason.
    CutOff(Option<CancelReason>),
}

type StreamEndFn = Box<dyn FnOnce(StreamOutcome) + Send>;

/// The state every handle to one execution shares.
pub(crate) struct ExecShared {
    pub(crate) app: Arc<AppShared>,
    /// The module the execution opened in: the dispatching controller's, the root for
    /// `App::execute`, `AppHandle::execute` and a transport that routes later, `M` for
    /// `ModuleRef::execute`.
    pub(crate) module: ModuleId,
    /// Set once by `Execution::route_to`; from then on it is the visibility every new resolver
    /// takes. A resolver built before keeps the module it was built with.
    pub(crate) routed: OnceLock<ModuleId>,
    pub(crate) cache: OnceCells,
    pub(crate) extensions: Extensions,
    pub(crate) inputs: Seeded,
    pub(crate) cancel: Arc<Notify>,
    /// Written once, before `cancel` fires, so a listener woken by the fire reads it.
    pub(crate) cancel_reason: OnceLock<CancelReason>,
    pub(crate) deadline: Option<Instant>,
    /// What `ExecOptions::trace` seeded, `Received`; `None` under `TraceMode::Off`.
    pub(crate) trace_inbound: Option<TraceContext>,
    /// Set once by `Execution::set_trace`, `Originated`; from then on what `trace()` answers.
    pub(crate) trace_own: OnceLock<TraceContext>,
    /// Set by `dispatch` before the first guard runs.
    pub(crate) handler: OnceLock<Arc<HandlerInfo>>,
    /// Set by `dispatch_late`: the error handlers now running see an error after the reply began.
    pub(crate) late: AtomicBool,
    pub(crate) stream_end: Mutex<StreamEnd>,
    /// Opened with `open_terminal`; counted in `Shutdown::terminal_skipped` when abandoned.
    pub(crate) terminal: bool,
    /// Set as the `Execution` drops, before the cache is released.
    pub(crate) ended: AtomicBool,
}

/// The reply stream's end: the outcome once reported, and the callbacks waiting for it.
#[derive(Default)]
pub(crate) struct StreamEnd {
    pub(crate) outcome: Option<StreamOutcome>,
    pub(crate) callbacks: Vec<StreamEndFn>,
}

impl Execution {
    /// Opens an execution resolving with `module`'s visibility. Refused from Draining on with
    /// `Closed`. For transports and standalone code alike.
    pub fn open(module: &ModuleRef, opts: ExecOptions) -> Result<Execution, Closed> {
        module.app.open(module.module, opts, false)
    }

    /// Opens a connection's cleanup during the drain: allowed in Draining, counted in the drain
    /// and bounded by `drain_timeout` like any other execution. The token proves a transport
    /// opens it; the phase check decides whether it may, so a token used after the drain has
    /// ended gets `Closed`.
    pub fn open_terminal(token: &DrainToken, module: &ModuleRef, opts: ExecOptions) -> Result<Execution, Closed> {
        let _ = token;
        module.app.open(module.module, opts, true)
    }

    /// Seeds an execution input. Inputs are `Send + Sync`, like everything the execution holds.
    pub fn seed<T: Send + Sync + 'static>(&self, input: T) {
        self.shared.inputs.insert(input);
    }

    /// Switches the execution to `module`'s visibility (transports DESIGN §2.10, X8). For a
    /// transport whose pre-dispatch stage runs before the route is known: it opens at the root,
    /// runs the unscoped entries, matches the route, then routes here before `dispatch`.
    ///
    /// Extensions written before the switch survive it. A resolver created before it keeps the
    /// root module, and a key that names a different binding under `module` is a second instance
    /// in the one execution, since instances sit in the cache under their binding.
    ///
    /// Takes effect once: a later call, or a `module` of another app, changes nothing.
    pub fn route_to(&self, module: &ModuleRef) {
        if Arc::ptr_eq(&module.app, &self.shared.app) {
            let _ = self.shared.routed.set(module.module);
        }
    }

    /// Records the execution's own trace context, a `TraceSource`'s answer, marked `Originated`:
    /// from then on [`trace`](Self::trace) answers it, and a client called `.within` this
    /// execution writes it out as a context originated here (integrations §6.3, I13).
    ///
    /// For whoever holds the execution, before `dispatch`: `ulo_transport::span::call` calls it
    /// between creating the call's span and entering it, and a standalone `execute` given a span
    /// does. Takes effect once; a later call, a call once `dispatch` has begun, or a call under
    /// `TraceMode::Off` changes nothing, so a guard or a handler never sees the context change.
    pub fn set_trace(&self, ctx: TraceContext) {
        self.shared.set_trace(ctx);
    }

    /// A cheap clone, for a guard, a reply stream or a spawned subtask. It does not keep the
    /// execution open: a subtask that should finish within the execution is awaited by its holder.
    pub fn handle(&self) -> ExecutionRef {
        ExecutionRef { shared: Arc::clone(&self.shared) }
    }

    pub fn extensions(&self) -> &Extensions {
        &self.shared.extensions
    }

    pub fn cancelled(&self) -> Cancelled<'_> {
        self.shared.cancelled()
    }

    pub fn is_cancelled(&self) -> bool {
        self.shared.cancel.is_fired()
    }

    /// Resolves as the drain begins (§9.5), a moment before the servers stop accepting; false
    /// through the before-shutdown stage. No execution opens from then on: a request arriving in
    /// that moment is refused, not served.
    pub fn draining(&self) -> Draining<'_> {
        self.shared.draining()
    }

    pub fn is_draining(&self) -> bool {
        self.shared.is_draining()
    }

    pub fn deadline(&self) -> Option<Instant> {
        self.shared.deadline
    }

    /// The execution's trace context: its own once `set_trace` recorded one, the inbound one
    /// `ExecOptions::trace` seeded before that, and `None` when neither exists. A client called
    /// `.within` this execution forwards it, as it forwards the deadline (integrations §6.1).
    pub fn trace(&self) -> Option<&TraceContext> {
        self.shared.trace()
    }

    /// `cancel_with(CancelReason::Explicit)`. Idempotent.
    pub fn cancel(&self) {
        self.shared.cancel_with(CancelReason::Explicit);
    }

    /// Fires cancellation with `reason`: the transport calls it when the client disconnects, sends
    /// a cancel frame or the deadline passes. The first reason written stays.
    pub fn cancel_with(&self, reason: CancelReason) {
        self.shared.cancel_with(reason);
    }

    /// The reason cancellation fired with; `None` while it has not.
    pub fn cancel_reason(&self) -> Option<CancelReason> {
        self.shared.cancel_reason.get().copied()
    }

    /// Registers `f` to run once, synchronously, when the reply stream finishes, with how it
    /// ended. Runtime-free and callable from an interceptor, which returns the reply before the
    /// stream is consumed. Registered after the end, `f` runs at once with the recorded outcome.
    /// A reply that never streams never runs it. A panic in `f` is caught and dropped wherever it
    /// runs; the callbacks after it still run.
    ///
    /// It reports the reply actually written; a stream discarded before the response is sent is
    /// not a reply and reports nothing. The first report an execution receives is the one kept.
    pub fn on_stream_end(&self, f: impl FnOnce(StreamOutcome) + Send + 'static) {
        self.shared.on_stream_end(Box::new(f));
    }

    /// The handler `dispatch` is running for this execution; `None` before `dispatch` and in a
    /// standalone execution.
    pub fn handler(&self) -> Option<&HandlerInfo> {
        self.shared.handler.get().map(|info| &**info)
    }

    /// True while the error handlers run on the late path: the error arrived after the reply
    /// began streaming, so no status can change (transports DESIGN §2.3, X7).
    pub fn is_late(&self) -> bool {
        self.shared.late.load(Ordering::Acquire)
    }

    /// `T` with this execution's module visibility, built in this execution if it is
    /// execution-scoped.
    pub async fn get<T: ?Sized + Send + Sync + 'static>(&self) -> Result<Dep<T>, LookupError> {
        self.resolver().dep::<T>().await
    }

    /// The resolver a transport reads through, for `Resolver::entries` and
    /// `FromContainer::from_container`.
    pub fn resolver(&self) -> Resolver<'_> {
        self.shared.resolver()
    }
}

impl ExecutionRef {
    pub fn handle(&self) -> ExecutionRef {
        self.clone()
    }

    pub fn extensions(&self) -> &Extensions {
        &self.shared.extensions
    }

    /// Owned, unlike [`Execution::cancelled`]: the future borrows nothing from the handle, so a
    /// stream built from a handle the handler took by value can carry it.
    pub fn cancelled(&self) -> Cancelled<'static> {
        Cancelled::new(self.shared.cancel.listen_owned())
    }

    pub fn is_cancelled(&self) -> bool {
        self.shared.cancel.is_fired()
    }

    /// Owned, like [`cancelled`](Self::cancelled):
    /// `stream.take_until(exec.draining())` ends a reply stream at the drain.
    pub fn draining(&self) -> Draining<'static> {
        Draining::new(self.shared.app.draining.listen_owned())
    }

    pub fn is_draining(&self) -> bool {
        self.shared.is_draining()
    }

    pub fn deadline(&self) -> Option<Instant> {
        self.shared.deadline
    }

    /// See [`Execution::trace`].
    pub fn trace(&self) -> Option<&TraceContext> {
        self.shared.trace()
    }

    pub fn cancel(&self) {
        self.shared.cancel_with(CancelReason::Explicit);
    }

    /// See [`Execution::cancel_with`]. On a handle as well, since a disconnect is noticed in a
    /// task holding a clone.
    pub fn cancel_with(&self, reason: CancelReason) {
        self.shared.cancel_with(reason);
    }

    pub fn cancel_reason(&self) -> Option<CancelReason> {
        self.shared.cancel_reason.get().copied()
    }

    pub fn on_stream_end(&self, f: impl FnOnce(StreamOutcome) + Send + 'static) {
        self.shared.on_stream_end(Box::new(f));
    }

    /// Records how the reply stream ended and runs every `on_stream_end` callback once, in the
    /// order registered. For `ulo_transport::Tracked`, which wraps every streaming answer and
    /// reports from its `Drop`; a later report changes nothing. A callback's panic is caught, so
    /// the report never panics.
    pub fn report_stream_end(&self, outcome: StreamOutcome) {
        self.shared.report_stream_end(outcome);
    }

    pub fn handler(&self) -> Option<&HandlerInfo> {
        self.shared.handler.get().map(|info| &**info)
    }

    pub fn is_late(&self) -> bool {
        self.shared.late.load(Ordering::Acquire)
    }

    /// As [`Execution::get`], while the execution lasts; once it has ended,
    /// `LookupError::ExecutionEnded` naming `T`, whatever `T`'s scope.
    pub async fn get<T: ?Sized + Send + Sync + 'static>(&self) -> Result<Dep<T>, LookupError> {
        if self.shared.ended.load(Ordering::Acquire) {
            return Err(LookupError::ExecutionEnded { key: Key::of::<T, ()>().name(BindingKind::Single) });
        }
        self.resolver().dep::<T>().await
    }

    /// The resolver a transport reads through. Once the execution has ended, every read through
    /// it answers `LookupError::ExecutionEnded`, as [`get`](Self::get) does.
    pub fn resolver(&self) -> Resolver<'_> {
        self.shared.resolver()
    }
}

impl Drop for Execution {
    fn drop(&mut self) {
        self.shared.ended.store(true, Ordering::Release);
        self.shared.cache.release();
    }
}

impl ExecShared {
    /// The visibility a new resolver takes: the routed module once `route_to` ran, the module the
    /// execution opened in until then.
    pub(crate) fn module(&self) -> ModuleId {
        self.routed.get().copied().unwrap_or(self.module)
    }

    pub(crate) fn resolver(self: &Arc<Self>) -> Resolver<'_> {
        Resolver::new(&self.app, self.module(), Some(self), Purpose::Lookup)
    }

    pub(crate) fn trace(&self) -> Option<&TraceContext> {
        self.trace_own.get().or(self.trace_inbound.as_ref())
    }

    /// `dispatch` sets `handler` before the first guard runs, which is where the holder's window
    /// closes.
    pub(crate) fn set_trace(&self, ctx: TraceContext) {
        if self.app.config.trace == TraceMode::Off || self.handler.get().is_some() {
            return;
        }
        let _ = self.trace_own.set(ctx.originated());
    }

    pub(crate) fn cancelled(&self) -> Cancelled<'_> {
        Cancelled::new(self.cancel.listen())
    }

    pub(crate) fn draining(&self) -> Draining<'_> {
        Draining::new(self.app.draining.listen())
    }

    pub(crate) fn is_draining(&self) -> bool {
        self.app.draining.is_fired()
    }

    /// The reason first, then the fire: a listener woken by the fire always reads a reason.
    pub(crate) fn cancel_with(&self, reason: CancelReason) {
        let _ = self.cancel_reason.set(reason);
        self.cancel.fire();
    }

    pub(crate) fn on_stream_end(&self, f: StreamEndFn) {
        let mut end = self.stream_end.lock().unwrap_or_else(PoisonError::into_inner);
        match end.outcome {
            Some(outcome) => {
                drop(end);
                run_stream_end(f, outcome);
            }
            None => end.callbacks.push(f),
        }
    }

    /// The callbacks run outside the lock: one may register another.
    pub(crate) fn report_stream_end(&self, outcome: StreamOutcome) {
        let callbacks = {
            let mut end = self.stream_end.lock().unwrap_or_else(PoisonError::into_inner);
            if end.outcome.is_some() {
                return;
            }
            end.outcome = Some(outcome);
            std::mem::take(&mut end.callbacks)
        };
        for callback in callbacks {
            run_stream_end(callback, outcome);
        }
    }
}

/// Runs one `on_stream_end` callback, dropping its panic. `Tracked` reports from its `Drop`, where
/// a panic during an unwind aborts the process and any other panic skips the callbacks after it.
/// The panic hook has already reported the panic by the time it is caught.
fn run_stream_end(callback: StreamEndFn, outcome: StreamOutcome) {
    let _ = catch_unwind(AssertUnwindSafe(move || callback(outcome)));
}
