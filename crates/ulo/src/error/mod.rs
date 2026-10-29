//! Every public error type (§10.2). Each is `#[non_exhaustive]`, the structs included: code
//! outside the core reads these types and builds none of them. `Redacted` carries no attribute;
//! its private fields close it the same way.
//!
//! The outcomes a caller propagates out of `main`, `StartupError`, `LoadError`, `ShutdownError`
//! and `WiringErrors`, write their `Display` text under `Debug` too, so `main` returning
//! `Box<dyn Error>` prints the report rather than the structure.

pub(crate) mod configure;
pub(crate) mod wiring;

use std::collections::HashSet;
use std::error::Error;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use crate::construct::ConstructError;
use crate::hooks::HookKind;
use crate::key::{BindingKind, KeyName};
use crate::module::{ModuleName, colliding_names};
use crate::redact::Redacted;
use crate::signal::Signal;
use crate::timer::BoxError;
use crate::type_name::{TypeName, short_type_name};

pub use configure::{ConfigureError, ConfigureErrors, PrepareEntry, PrepareError};
pub use wiring::WiringErrors;

#[non_exhaustive]
pub enum StartupError {
    /// Everything from `wire()`.
    Wiring(WiringErrors),
    Connect(ConnectError),
    /// Every failure a server's `prepare` reported, collected across every server before any
    /// binds, so a configuration error is always reported before a port conflict.
    Configure(ConfigureErrors),
    /// A transport's own error, or the core's `RuntimeMissing`. The one type name it prints,
    /// `transport`, has nothing to collide with and prints short.
    Bind { transport: TypeName, source: Redacted },
}

impl fmt::Display for StartupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StartupError::Wiring(e) => fmt::Display::fmt(e, f),
            StartupError::Connect(e) => fmt::Display::fmt(e, f),
            StartupError::Configure(e) => fmt::Display::fmt(e, f),
            StartupError::Bind { transport, source } => {
                write!(f, "transport `{transport}` failed to bind: {source}")
            }
        }
    }
}

impl fmt::Debug for StartupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

/// Transparent: the wrapped error's text is this error's text, so its source is the wrapped
/// error's source rather than the wrapped error, which a chain reporter would print twice.
impl Error for StartupError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            StartupError::Wiring(e) => e.source(),
            StartupError::Connect(e) => e.source(),
            StartupError::Configure(e) => e.source(),
            StartupError::Bind { .. } => None,
        }
    }
}

impl From<WiringErrors> for StartupError {
    fn from(e: WiringErrors) -> Self {
        StartupError::Wiring(e)
    }
}

impl From<ConnectError> for StartupError {
    fn from(e: ConnectError) -> Self {
        StartupError::Connect(e)
    }
}

impl From<ConfigureErrors> for StartupError {
    fn from(e: ConfigureErrors) -> Self {
        StartupError::Configure(e)
    }
}

/// `listen()`'s refusal of an app that binds a transport with no `Runtime` (§9.5), stored in
/// `StartupError::Bind`'s `source` like a transport's error;
/// `source.downcast_ref::<RuntimeMissing>()` tells it from a port already taken.
///
/// Every transport spawns its connections and calls on the app's runtime and times them on its
/// clock, so an app given `.timer(..)` alone is refused too; one that binds no transport, a job
/// or a CLI command, needs only the clock.
#[non_exhaustive]
#[derive(Debug)]
pub struct RuntimeMissing {
    pub transport: TypeName,
}

impl fmt::Display for RuntimeMissing {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "transport `{}` is bound on an app with no `Runtime`; set one with `.runtime(..)`, which sets the app's timer too",
            self.transport
        )
    }
}

impl Error for RuntimeMissing {}

/// A server's `prepare` that panicked, caught by `listen()` and reported as that server's
/// [`ConfigureError`], whose `source` holds it. The other servers' `prepare` still run, and
/// nothing binds. [`is_panic`] recognises it.
#[non_exhaustive]
#[derive(Debug)]
pub struct PreparePanicked {
    /// The payload, converted to a message and redacted.
    pub message: Redacted,
}

impl fmt::Display for PreparePanicked {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "`prepare` panicked: {}", self.message)
    }
}

impl Error for PreparePanicked {}

/// A failure in the connect phase, on `StartupError::Connect` from `connect` and on
/// `LoadError::Connect` from `load`.
#[non_exhaustive]
#[derive(Debug)]
pub enum ConnectError {
    Construct { key: KeyName, module: ModuleName, reason: FailureReason },
    /// How the last attempt ended, `TimedOut { limit: Item }` for the whole check, or `Errored`
    /// holding a `Permanent` [`RuntimeShutDown`](crate::RuntimeShutDown) for a backoff or a bound
    /// the runtime refused (§9.3).
    Readiness { key: KeyName, attempts: u32, reason: FailureReason },
    Hook { hook: HookKind, key: KeyName, reason: FailureReason },
}

impl fmt::Display for ConnectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConnectError::Construct { key, module, reason } => {
                write!(f, "failed to construct `{key}` in {module}: {reason}")
            }
            ConnectError::Readiness { key, attempts, reason } => {
                let noun = if *attempts == 1 { "attempt" } else { "attempts" };
                write!(f, "readiness check of `{key}` failed after {attempts} {noun}: {reason}")
            }
            ConnectError::Hook { hook, key, reason } => write!(f, "`{hook}` hook of `{key}` failed: {reason}"),
        }
    }
}

impl Error for ConnectError {}

/// Why a hook, a construction, a readiness check or a transport's `close` did not complete. One
/// enum for every place one can fail, so a timeout reads the same on an init hook, a destroy
/// hook, a constructor, a check and a `close`.
#[non_exhaustive]
#[derive(Debug)]
pub enum FailureReason {
    /// The payload, converted to a message.
    Panicked(Redacted),
    /// `after` is the configured duration of the limit that fired.
    TimedOut { after: Duration, limit: Limit },
    /// Never started: `shutdown_timeout` had expired (§9.5); shutdown hooks only.
    Skipped,
    /// Returned `Err`: a construction, an init or bootstrap hook, the last attempt of a
    /// readiness check, or a transport's `close`. Also a bound the `Timer` refused, its runtime
    /// shut down, holding a `Permanent` [`RuntimeShutDown`](crate::RuntimeShutDown).
    Errored(Redacted),
}

impl fmt::Display for FailureReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FailureReason::Panicked(message) => write!(f, "panicked: {message}"),
            FailureReason::TimedOut { after, limit } => {
                let which = match limit {
                    Limit::Item => "its own bound",
                    Limit::Attempt => "the attempt bound",
                    Limit::Default => "the app default",
                    Limit::ShutdownCap => "`shutdown_timeout`",
                };
                write!(f, "timed out after {after:?} ({which})")
            }
            FailureReason::Skipped => f.write_str("skipped: `shutdown_timeout` had expired"),
            FailureReason::Errored(error) => fmt::Display::fmt(error, f),
        }
    }
}

/// Which limit a `TimedOut` hit.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Limit {
    /// An explicit bound on the whole item: a trait const's `After(..)`, or `.timeout(..)` on a
    /// binding handle (§3.9).
    Item,
    /// An explicit `.attempt_timeout(..)` on a readiness check (§9.3).
    Attempt,
    /// The app default for this kind of item: `hook_timeout` for a hook and for a transport's
    /// `close` when no cap is set, `construct_timeout` for a construction and for one attempt of
    /// a readiness check.
    Default,
    /// `shutdown_timeout` cutting a hook or a transport's `close` mid-run (§9.5).
    ShutdownCap,
}

/// The outcome of a shutdown, received by `serve` and by every `close` caller (§9.5).
#[non_exhaustive]
#[derive(Clone, Debug)]
pub struct Shutdown {
    /// The trigger that won.
    pub signal: Signal,
    /// Executions still alive at the drain's end: `drain_timeout`, or the earlier
    /// `shutdown_timeout` expiry.
    pub abandoned: usize,
    /// Terminal executions refused or abandoned (§9.5).
    pub terminal_skipped: usize,
}

/// One outcome, several receivers; the failures sit behind the `Arc`.
///
/// `Display` applies the wiring report's naming rule across every failure: two different types
/// printing alike anywhere in the report, a key's type or qualifier or a transport, print with
/// their full paths. A [`ShutdownFailure`] displayed on its own applies it to its own names.
#[non_exhaustive]
#[derive(Clone)]
pub struct ShutdownError {
    pub report: Shutdown,
    pub failures: Arc<[ShutdownFailure]>,
}

impl fmt::Display for ShutdownError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let count = self.failures.len();
        let noun = if count == 1 { "failure" } else { "failures" };
        write!(f, "shutdown on `{}` finished with {count} {noun}", self.report.signal)?;
        let full = TypeName::colliding(self.failures.iter().flat_map(ShutdownFailure::type_names));
        for failure in self.failures.iter() {
            write!(f, "\n  × {}", failure.text(&full))?;
        }
        let abandoned = self.report.abandoned;
        if abandoned > 0 {
            let noun = if abandoned == 1 { "execution" } else { "executions" };
            write!(f, "\n  {abandoned} {noun} abandoned at the drain's end")?;
        }
        let skipped = self.report.terminal_skipped;
        if skipped > 0 {
            let noun = if skipped == 1 { "terminal execution" } else { "terminal executions" };
            write!(f, "\n  {skipped} {noun} refused or abandoned")?;
        }
        Ok(())
    }
}

impl fmt::Debug for ShutdownError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl Error for ShutdownError {}

#[non_exhaustive]
#[derive(Debug)]
pub enum ShutdownFailure {
    /// `Errored` only for a closure hook whose dependency read failed: the shutdown hook traits and
    /// closures return `()`.
    Hook { hook: HookKind, key: KeyName, reason: FailureReason },
    /// `Errored` for the transport's own error, `Panicked` for a panic in its `close`, and
    /// `TimedOut` for a `close` exceeding its bound (§9.5): `limit: ShutdownCap` with the cap's
    /// duration when the cap bounded it, `limit: Default` with `hook_timeout` otherwise. A bound
    /// the `Timer` refused is `Errored` holding a `Permanent` `RuntimeShutDown`. Never `Skipped`:
    /// every `close` starts.
    Close { transport: TypeName, reason: FailureReason },
    /// The `App` value dropped before its shutdown's close step ended, with no `close` holding a
    /// claim on the servers, and the drop released them without their `close`. The `close`
    /// answering it ran none of the shutdown's steps: no hook runs once the app has dropped this
    /// way, and the report counts no execution.
    Dropped,
    /// The transport's `serve` ended before any other trigger arrived, and that end started this
    /// shutdown (§9.5). It is the first failure listed; the steps that follow add theirs.
    Serve { transport: TypeName, end: ServeEnd },
}

/// How a transport's `serve` ended while the app was still running, on [`ShutdownFailure::Serve`].
#[non_exhaustive]
#[derive(Debug)]
pub enum ServeEnd {
    /// Returned `Err`: the transport's own error, redacted.
    Errored(Redacted),
    /// Returned `Ok` before the shutdown began, so nothing served that transport any more.
    Returned,
}

impl ShutdownFailure {
    /// Every type name this failure prints, for the report's collision check.
    fn type_names(&self) -> Vec<TypeName> {
        match self {
            ShutdownFailure::Hook { key, .. } => key.key().type_names().collect(),
            ShutdownFailure::Close { transport, .. } => vec![*transport],
            ShutdownFailure::Dropped => Vec::new(),
            ShutdownFailure::Serve { transport, .. } => vec![*transport],
        }
    }

    /// The failure, each type name in `full` written with its full path.
    fn text(&self, full: &HashSet<TypeName>) -> String {
        match self {
            ShutdownFailure::Hook { hook, key, reason } => format!("`{hook}` hook of `{}`: {reason}", key.text_in(full)),
            ShutdownFailure::Close { transport, reason } => {
                format!("transport `{}` failed to close: {reason}", transport.written(full))
            }
            ShutdownFailure::Dropped => "the app was dropped before its shutdown closed its servers: they were released \
                                         without their `close`, and no shutdown hook runs after the drop"
                .to_owned(),
            ShutdownFailure::Serve { transport, end: ServeEnd::Errored(reason) } => {
                format!("transport `{}` failed while serving: {reason}", transport.written(full))
            }
            ShutdownFailure::Serve { transport, end: ServeEnd::Returned } => {
                format!("transport `{}` stopped serving while the app was running", transport.written(full))
            }
        }
    }
}

impl fmt::Display for ShutdownFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text(&TypeName::colliding(self.type_names())))
    }
}

/// `execute` and `Execution::open` from Draining on; `load` carries it in `LoadError::Closed`
/// from Stopping on (§9.5). Constructed by the core alone.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Closed;

impl Closed {
    pub(crate) const fn new() -> Self {
        Closed
    }
}

impl fmt::Display for Closed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("the application is shutting down")
    }
}

impl Error for Closed {}

/// Why `load` did not return a `ModuleRef` (§8.6).
#[non_exhaustive]
pub enum LoadError {
    Closed(Closed),
    /// The module wired against the frozen graph, every error collected.
    Wiring(WiringErrors),
    /// A construction, readiness check or init hook of the module failed.
    Connect(ConnectError),
    Refused(LoadRefusal),
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoadError::Closed(closed) => write!(f, "cannot load a module: {closed}"),
            LoadError::Wiring(e) => fmt::Display::fmt(e, f),
            LoadError::Connect(e) => fmt::Display::fmt(e, f),
            LoadError::Refused(refusal) => fmt::Display::fmt(refusal, f),
        }
    }
}

impl fmt::Debug for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

/// Transparent, like `StartupError`.
impl Error for LoadError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            LoadError::Closed(_) | LoadError::Refused(_) => None,
            LoadError::Wiring(e) => e.source(),
            LoadError::Connect(e) => e.source(),
        }
    }
}

/// What a lazily loaded module may not bring, because the graph has been handed out (§8.6).
#[non_exhaustive]
#[derive(Debug)]
pub enum LoadRefusal {
    Controllers { module: ModuleName },
    Middleware { module: ModuleName },
    /// To a collection the module does not introduce.
    Contribution { module: ModuleName, key: KeyName },
    GlobalExport { module: ModuleName, key: KeyName },
    /// Inputs belong to transports; the per-handler check ran at wiring time (§6.4).
    Input { module: ModuleName, key: KeyName },
}

impl fmt::Display for LoadRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoadRefusal::Controllers { module } => {
                write!(f, "cannot load {module}: it declares controllers, and routes are already bound")
            }
            LoadRefusal::Middleware { module } => write!(
                f,
                "cannot load {module}: it writes module metadata such as middleware, which transports read when they bind"
            ),
            LoadRefusal::Contribution { module, key } => write!(
                f,
                "cannot load {module}: it contributes to `{key}`, a collection it does not introduce, which bindings already read"
            ),
            LoadRefusal::GlobalExport { module, key } => {
                write!(f, "cannot load {module}: it exports `{key}` globally, and global exports are fixed at wiring")
            }
            LoadRefusal::Input { module, key } => write!(
                f,
                "cannot load {module}: it declares the execution input `{key}`; inputs belong to transports, and the per-handler input check ran at wiring"
            ),
        }
    }
}

#[non_exhaustive]
#[derive(Debug)]
pub enum LookupError {
    /// A binding, an extension, an input or a module.
    NotFound { key: KeyName, kind: LookupKind },
    /// A singleton the connect walk has not built yet (§8.5).
    NotReady { key: KeyName },
    /// An erased `Key` asked for a `T` it does not hold.
    WrongType { key: KeyName, requested: &'static str },
    /// Single read as collection, or the reverse.
    WrongKind { key: KeyName, expected: BindingKind, found: BindingKind },
    ExecutionRequired { key: KeyName },
    /// A read through an execution that has ended, its `Execution` dropped (§3.8): from a
    /// subtask's `ExecutionRef`, or from a build that began before the end.
    ExecutionEnded { key: KeyName },
    /// A runtime lookup of a key two visible exports answer (§8.2): `ModuleRef::get`, `get` in
    /// an execution opened on a module other than the root, or `by_key`. `sources` are the
    /// exporting modules. `Option<S>` propagates it.
    Ambiguous { key: KeyName, sources: Vec<ModuleName> },
    /// `app.module::<M>()` over several instances of `M` (§8.5); `module` is `M`'s type name.
    AmbiguousModule { module: &'static str, candidates: Vec<ModuleName> },
    /// A build inside a call failed, panicked or timed out (§3.4).
    Construct { key: KeyName, reason: FailureReason },
    /// A singleton lookup from Destroying on (§9.5).
    Closed { key: KeyName },
}

impl fmt::Display for LookupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LookupError::NotFound { key, kind } => match kind {
                LookupKind::Binding => write!(f, "no binding for `{key}` is visible from this module"),
                LookupKind::Extension => write!(f, "the extension `{key}` was not written in this execution"),
                LookupKind::Input => write!(f, "the execution input `{key}` was not seeded in this execution"),
                LookupKind::Module => write!(f, "no module `{key}` is registered"),
            },
            LookupError::NotReady { key } => write!(f, "`{key}` is a singleton the connect walk has not built yet"),
            LookupError::WrongType { key, requested } => {
                write!(f, "`{key}` does not hold a `{}`", short_type_name(*requested))
            }
            LookupError::WrongKind { key, expected, found } => {
                write!(f, "`{key}` is {}, read as {}", kind_text(*found), kind_text(*expected))
            }
            LookupError::ExecutionRequired { key } => write!(f, "`{key}` needs an execution, and none is open here"),
            LookupError::ExecutionEnded { key } => write!(f, "`{key}` was read through an execution that has ended"),
            LookupError::Ambiguous { key, sources } => {
                write!(f, "`{key}` is ambiguous between ")?;
                write_list(f, sources)
            }
            LookupError::AmbiguousModule { module, candidates } => {
                write!(f, "`{}` is ambiguous between ", short_type_name(*module))?;
                write_list(f, candidates)
            }
            LookupError::Construct { key, reason } => write!(f, "building `{key}` inside the call failed: {reason}"),
            LookupError::Closed { key } => {
                write!(f, "`{key}` is a singleton, and the application has begun destroying its singletons")
            }
        }
    }
}

impl Error for LookupError {}

fn write_list(f: &mut fmt::Formatter<'_>, names: &[ModuleName]) -> fmt::Result {
    let colliding = colliding_names(names);
    for (i, name) in names.iter().enumerate() {
        if i > 0 {
            f.write_str(", ")?;
        }
        if colliding.contains(name) {
            write!(f, "{name:#}")?;
        } else {
            write!(f, "{name}")?;
        }
    }
    Ok(())
}

fn kind_text(kind: BindingKind) -> &'static str {
    match kind {
        BindingKind::Single => "a single binding",
        BindingKind::Collection => "a collection",
    }
}

/// What a `LookupError::NotFound` did not find.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LookupKind {
    Binding,
    Extension,
    Input,
    /// `app.module::<M>()` with no module of type `M` registered.
    Module,
}

/// A guard's `Ok(false)`, as the error handlers see it (§7). Unclaimed, the transport renders
/// its forbidden status.
#[non_exhaustive]
#[derive(Clone, Copy, Debug)]
pub struct GuardRejected {
    pub guard: &'static str,
}

impl GuardRejected {
    pub(crate) fn new(guard: &'static str) -> Self {
        GuardRejected { guard }
    }
}

impl fmt::Display for GuardRejected {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "guard `{}` refused the call", self.guard)
    }
}

impl Error for GuardRejected {}

/// A panic in the pipeline, caught by `dispatch` and offered to the error handlers as the
/// call's error (§7). Unclaimed, the transport renders its internal-error status.
#[non_exhaustive]
#[derive(Debug)]
pub struct PanicRecovered {
    pub stage: DispatchStage,
    /// The payload, converted to a message.
    pub message: Redacted,
}

impl PanicRecovered {
    pub(crate) fn new(stage: DispatchStage, message: Redacted) -> Self {
        PanicRecovered { stage, message }
    }
}

impl fmt::Display for PanicRecovered {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "the {} panicked: {}", self.stage, self.message)
    }
}

impl Error for PanicRecovered {}

/// Which stage of `dispatch` panicked.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DispatchStage {
    Guard,
    Interceptor,
    Handler,
    ErrorHandler,
    /// A transport's pre-dispatch stage: a middleware or a tower layer, caught by the transport
    /// through [`AppHandle::catch_panic`](crate::AppHandle::catch_panic).
    PreDispatch,
}

impl fmt::Display for DispatchStage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            DispatchStage::Guard => "guard",
            DispatchStage::Interceptor => "interceptor",
            DispatchStage::Handler => "handler",
            DispatchStage::ErrorHandler => "error handler",
            DispatchStage::PreDispatch => "pre-dispatch stage",
        })
    }
}

/// The sentinel an error handler returns on the late path to end a stream cleanly, as if it had
/// returned `None` (transports DESIGN §2.3, X7). Anywhere else it is an ordinary error.
///
/// An `Ok` from a handler on the late path is ignored rather than read as a clean end: an `Ok`
/// written for the pre-stream case is a failure rendering, and ending the stream cleanly on it
/// would tell the client the stream completed when it failed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EndStream;

impl fmt::Display for EndStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("the stream ends here")
    }
}

impl Error for EndStream {}

/// Marks a failure that retrying this start, with this configuration on this runtime, cannot mend:
/// bad credentials, an unknown database, a runtime the integration does not support, a migration
/// that failed, a runtime that has shut down ([`RuntimeShutDown`](crate::RuntimeShutDown)). A
/// process restart brings a new runtime, which is a different start. A readiness attempt whose
/// error is one ends the check at once, with no further attempt and no backoff, as
/// `ConnectError::Readiness { reason: Errored(..) }`; [`is_permanent`] finds one in a
/// `StartupError`, so `main` can tell a start that retrying cannot mend from one it might.
///
/// An integration wraps its own error, `Err(Permanent::new(e).into())`, wherever it returns a
/// `BoxError`. `Display` and `source()` are the wrapped error's, so the report reads as the error
/// itself; the marker adds no text.
#[non_exhaustive]
pub struct Permanent(BoxError);

impl Permanent {
    pub fn new(error: impl Into<BoxError>) -> Permanent {
        Permanent(error.into())
    }
}

impl fmt::Debug for Permanent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Permanent").field(&self.0).finish()
    }
}

impl fmt::Display for Permanent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

/// The wrapped error, so a reporter walking the chain reads its cause after the marker's text.
impl Error for Permanent {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&*self.0)
    }
}

/// Whether retrying this start, with this configuration on this runtime, cannot succeed: the
/// failure carries a [`Permanent`], or it is a misconfiguration by its phase. A process restart
/// brings a new runtime, which is a different start.
///
/// - `Wiring` is always permanent: the graph and the values registered into it are fixed by the
///   code and its configuration, which a restart reads again unchanged.
/// - `Connect`, `Configure` and `Bind` are permanent when a `Permanent` is found under them: in a
///   construction's or a hook's error, in the last attempt of a readiness check, in a server's
///   `prepare` failure or a transport's bind error, and down any error's `source()` chain, inside a
///   [`Redacted`] too. `Bind` carrying the core's [`RuntimeMissing`] is permanent as well.
///
/// Anything else is retryable: a readiness check or a construction that timed out, a dependency
/// that answered with an error carrying no `Permanent`, a port already taken.
pub fn is_permanent(error: &StartupError) -> bool {
    startup_permanent(error, 0)
}

fn startup_permanent(error: &StartupError, depth: usize) -> bool {
    match error {
        StartupError::Wiring(_) => true,
        StartupError::Connect(connect) => connect_permanent(connect, depth),
        StartupError::Configure(errors) => errors.iter().any(|e| redacted_permanent(&e.source, depth + 1)),
        StartupError::Bind { source, .. } => {
            source.downcast_ref::<RuntimeMissing>().is_some() || redacted_permanent(source, depth + 1)
        }
    }
}

/// Whether a readiness attempt's error carries a [`Permanent`], read before the attempt's error is
/// redacted, which keeps the original reachable but by type alone.
pub(crate) fn construct_permanent(error: &ConstructError) -> bool {
    match error {
        ConstructError::Failed(inner) => permanent_in(&**inner, 1),
        ConstructError::Dependency(lookup) => lookup_permanent(lookup, 1),
    }
}

fn permanent_in(error: &(dyn Error + 'static), depth: usize) -> bool {
    if depth >= MAX_DEPTH {
        return false;
    }
    if error.downcast_ref::<Permanent>().is_some() {
        return true;
    }
    if let Some(redacted) = error.downcast_ref::<Redacted>() {
        return redacted_permanent(redacted, depth + 1);
    }
    if let Some(found) = recognized_permanent(error, depth) {
        return found;
    }
    error.source().is_some_and(|cause| permanent_in(cause, depth + 1))
}

/// A `Redacted` answers `None` for `source()`, so its original is walked instead: the core's own
/// types by their structure, and any other error down its own chain.
fn redacted_permanent(redacted: &Redacted, depth: usize) -> bool {
    depth < MAX_DEPTH && permanent_in(redacted.original(), depth)
}

/// `Some` for the core's types that hold their failures in fields `source()` does not reach.
fn recognized_permanent(error: &(dyn Error + 'static), depth: usize) -> Option<bool> {
    if let Some(lookup) = error.downcast_ref::<LookupError>() {
        return Some(lookup_permanent(lookup, depth));
    }
    if let Some(construct) = error.downcast_ref::<ConstructError>() {
        return Some(match construct {
            ConstructError::Dependency(lookup) => lookup_permanent(lookup, depth),
            ConstructError::Failed(inner) => permanent_in(&**inner, depth + 1),
        });
    }
    if let Some(connect) = error.downcast_ref::<ConnectError>() {
        return Some(connect_permanent(connect, depth));
    }
    if let Some(load) = error.downcast_ref::<LoadError>() {
        return Some(matches!(load, LoadError::Connect(connect) if connect_permanent(connect, depth)));
    }
    if let Some(startup) = error.downcast_ref::<StartupError>() {
        return Some(startup_permanent(startup, depth));
    }
    if error.downcast_ref::<PrepareError>().is_some_and(PrepareError::is_permanent) {
        return Some(true);
    }
    None
}

fn lookup_permanent(error: &LookupError, depth: usize) -> bool {
    match error {
        LookupError::Construct { reason, .. } => reason_permanent(reason, depth),
        _ => false,
    }
}

fn connect_permanent(error: &ConnectError, depth: usize) -> bool {
    match error {
        ConnectError::Construct { reason, .. }
        | ConnectError::Readiness { reason, .. }
        | ConnectError::Hook { reason, .. } => reason_permanent(reason, depth),
    }
}

fn reason_permanent(reason: &FailureReason, depth: usize) -> bool {
    match reason {
        FailureReason::Errored(redacted) => redacted_permanent(redacted, depth + 1),
        FailureReason::Panicked(_) | FailureReason::TimedOut { .. } | FailureReason::Skipped => false,
    }
}

/// Whether `error` carries a panic the core caught, whichever shape it arrived in. Inside a call
/// there are two: [`PanicRecovered`] from a guard, an interceptor, the handler or an error
/// handler, and [`LookupError::Construct`] with `reason: Panicked` from a build the container
/// ran for the call, an enhancer, the controller or one of their dependencies. An error handler
/// that answers every panic alike tests this instead of matching both.
///
/// Also true for a `ConnectError` whose reason is `Panicked`, bare or inside `LoadError` or
/// `StartupError`, which is how a handler calling `load` meets a lazily loaded module's panic,
/// and for a [`PreparePanicked`] in an entry of `StartupError::Configure` or of the
/// [`ConfigureErrors`] it holds, which is how `listen()` reports a server's panicking `prepare`.
///
/// The panic is found under a constructor or a `try_` factory that returned a dependency's
/// `LookupError` as its own `Err`, and down any error's `source()` chain. Inside a [`Redacted`],
/// which hands its original out by type alone, only the core's error types are recognized: a
/// panic wrapped in another error type before it was redacted is not found.
pub fn is_panic(error: &BoxError) -> bool {
    panic_in(&**error, 0)
}

/// Bounded, in case a `source()` chain loops back on itself.
const MAX_DEPTH: usize = 32;

fn panic_in(error: &(dyn Error + 'static), depth: usize) -> bool {
    if depth >= MAX_DEPTH {
        return false;
    }
    match recognized(error, depth) {
        Some(found) => found,
        None => error.source().is_some_and(|cause| panic_in(cause, depth + 1)),
    }
}

fn redacted_panicked(redacted: &Redacted, depth: usize) -> bool {
    depth < MAX_DEPTH && recognized(redacted, depth).unwrap_or(false)
}

/// `Some` when `error` is one of the core's types that can carry a panic, holding whether it
/// does; `None` for any other type.
fn recognized<'e>(error: impl ByType<'e>, depth: usize) -> Option<bool> {
    if error.get::<PanicRecovered>().is_some() || error.get::<PreparePanicked>().is_some() {
        return Some(true);
    }
    if let Some(lookup) = error.get::<LookupError>() {
        return Some(lookup_panicked(lookup, depth));
    }
    if let Some(construct) = error.get::<ConstructError>() {
        return Some(match construct {
            ConstructError::Dependency(lookup) => lookup_panicked(lookup, depth),
            ConstructError::Failed(inner) => panic_in(&**inner, depth + 1),
        });
    }
    if let Some(connect) = error.get::<ConnectError>() {
        return Some(connect_panicked(connect, depth));
    }
    if let Some(load) = error.get::<LoadError>() {
        return Some(matches!(load, LoadError::Connect(connect) if connect_panicked(connect, depth)));
    }
    if let Some(startup) = error.get::<StartupError>() {
        return Some(match startup {
            StartupError::Connect(connect) => connect_panicked(connect, depth),
            StartupError::Configure(errors) => configure_panicked(errors, depth),
            StartupError::Wiring(_) | StartupError::Bind { .. } => false,
        });
    }
    if let Some(errors) = error.get::<ConfigureErrors>() {
        return Some(configure_panicked(errors, depth));
    }
    if let Some(redacted) = error.get::<Redacted>() {
        return Some(redacted_panicked(redacted, depth + 1));
    }
    None
}

/// Each entry's `source` is a `Redacted`, which `ConfigureError::source()` does not hand out:
/// `Display` already writes its text, so a chain reporter would print it twice. The entries are
/// walked here instead.
fn configure_panicked(errors: &ConfigureErrors, depth: usize) -> bool {
    errors.iter().any(|entry| redacted_panicked(&entry.source, depth + 1))
}

fn lookup_panicked(error: &LookupError, depth: usize) -> bool {
    match error {
        LookupError::Construct { reason, .. } => reason_panicked(reason, depth),
        _ => false,
    }
}

fn connect_panicked(error: &ConnectError, depth: usize) -> bool {
    match error {
        ConnectError::Construct { reason, .. }
        | ConnectError::Readiness { reason, .. }
        | ConnectError::Hook { reason, .. } => reason_panicked(reason, depth),
    }
}

fn reason_panicked(reason: &FailureReason, depth: usize) -> bool {
    match reason {
        FailureReason::Panicked(_) => true,
        FailureReason::Errored(redacted) => redacted_panicked(redacted, depth + 1),
        FailureReason::TimedOut { .. } | FailureReason::Skipped => false,
    }
}

/// The original by its type, which `dyn Error` and `Redacted` both answer.
trait ByType<'e> {
    fn get<E: Error + 'static>(&self) -> Option<&'e E>;
}

impl<'e> ByType<'e> for &'e (dyn Error + 'static) {
    fn get<E: Error + 'static>(&self) -> Option<&'e E> {
        let error: &'e (dyn Error + 'static) = *self;
        error.downcast_ref::<E>()
    }
}

impl<'e> ByType<'e> for &'e Redacted {
    fn get<E: Error + 'static>(&self) -> Option<&'e E> {
        let redacted: &'e Redacted = *self;
        redacted.downcast_ref::<E>()
    }
}
