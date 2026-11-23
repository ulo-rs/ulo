//! The `ulo` core: keys, bindings, injection points, scopes, executions, modules, lifecycle,
//! errors, and the enhancer traits a transport implements against.
//!
//! The core has no async runtime. It uses `std::future`, the [`BoxFuture`] alias, a pluggable
//! [`Timer`] and a pluggable [`Spawn`], the two together a [`Runtime`]; transports and runtime
//! adapters bring the executor, sockets, timers and signals.
//! Every macro expands to calls on the value-level API re-exported here, which integration
//! crates call directly.
//!
//! The macros name this crate as `::ulo`. A crate that depends on it under another name,
//! `framework = { package = "ulo", .. }` in its manifest, writes `extern crate framework as ulo;`
//! at its root for any `ulo` macro to expand.
//!
//! # Start here
//!
//! This page documents the core's types. To build an application, begin with the guide at
//! <https://ulo-rs.vercel.app/>. `cargo install ulo-cli --version 0.1.0-alpha.3` then
//! `ulo new my-app` writes a working project, and `examples/` in the repository holds one runnable
//! application per concept.
//!
//! A module, one controller with one route, and the runtime crate's `run`, which serves it
//! (`examples/01-hello`), with `ulo-http`, `ulo-http-hyper` and `ulo-runtime-tokio` beside `ulo`:
//!
//! ```no_run
//! use ulo::{controller, handlers, module};
//! use ulo_http_hyper::Server;
//!
//! #[controller]
//! pub struct HelloController;
//!
//! #[handlers]
//! impl HelloController {
//!     #[ulo_http::get("/")]
//!     fn hello(&self) -> &'static str {
//!         "Hello, ulo!"
//!     }
//! }
//!
//! #[module(controllers = [HelloController])]
//! pub struct AppModule;
//!
//! fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     ulo_runtime_tokio::run(AppModule, Server::new("127.0.0.1:3000"))?;
//!     Ok(())
//! }
//! ```
//!
//! `run` performs the phases of [`App`]: [`App::builder`] with the runtime, `wire`, `connect`,
//! `bind`, `listen` and `serve` until SIGINT or SIGTERM. An application that sets a builder knob
//! or reads the app between two phases writes them itself.

// The README blocks are compiled, not rendered: the `//!` docs above stay the crate's page.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;

#[cfg(doctest)]
#[doc = include_str!("../../../README.md")]
struct RootReadmeDoctests;

mod binding;
mod construct;
mod dependency;
mod error;
mod execution;
mod graph;
mod hooks;
mod key;
mod lifecycle;
mod module;
mod redact;
mod resolver;
mod runtime;
mod signal;
mod timer;
mod transport;
mod type_name;

pub mod app;
pub mod panic;
pub mod scope;
pub mod secrets;
pub mod testing;

/// The binding-handle typestate: the state markers `Handle`'s parameters take and the traits
/// that decide which methods exist in each state (§9.1). Also the marks on `Contribute`'s last
/// parameter: `Plain` on the builder `contribute` returns, `Enhancer` on the one `enhancer`
/// returns, which has no `qualified`.
pub mod handle {
    pub use crate::binding::contribute::{Enhancer, Plain};
    pub use crate::binding::handle::{
        AttemptTimeout, Binding, Contribution, Handle, HookHost, HookItem, Open, ReadyItem, Set,
        SingleBinding, Timeout, Unbounded,
    };
}

#[doc(hidden)]
pub mod __private;

pub use ulo_macros::{construct, controller, error_handlers, guards, handlers, injectable, interceptors, meta, module};

pub use app::{App, AppBuilder, AppHandle, Connected, Phase, Wired};
pub use binding::alias::{Alias, Input};
pub use binding::contribute::Contribute;
pub use binding::factory::{Factory, ShutdownFactory};
pub use binding::handle::Handle;
pub use construct::{Construct, ConstructError};
pub use dependency::{Dep, Dependencies, Ext, FromContainer, Many, Requirement};
pub use error::{
    Closed, ConfigureError, ConfigureErrors, ConnectError, DispatchStage, EndStream, FailureReason, GuardRejected,
    Limit, LoadError, LoadRefusal, LookupError, LookupKind, PanicRecovered, PrepareEntry, PrepareError, Shutdown, ShutdownError,
    Permanent, PreparePanicked, RuntimeMissing, ServeEnd, ShutdownFailure, StartupError, UndeclaredRead, Unreached, is_panic,
    is_permanent,
};
pub use error::wiring::{BoundItem, DifferingSetting, InputOrigin, ModuleSite, WiringError, WiringErrors};
pub use execution::extensions::Extensions;
pub use execution::notify::{Cancelled, Draining};
pub use execution::trace::{TraceContext, TraceMode, TraceOrigin, TraceSource, TraceState};
pub use execution::{CancelReason, ExecOptions, Execution, ExecutionRef, StreamOutcome};
pub use hooks::{
    BeforeApplicationShutdown, HookKind, Hooks, OnApplicationBootstrap, OnApplicationShutdown,
    OnModuleDestroy, OnModuleInit,
};
pub use key::{BindingKind, Key, KeyName};
pub use type_name::TypeName;
pub use module::dynamic::DynamicModule;
pub use module::handle::ModuleRef;
pub use module::keyed::Keyed;
pub use module::meta::Meta;
pub use module::def::{ModuleDef, ModuleHook};
pub use module::{Module, ModuleIdentity, ModuleName, ModuleSettings};
pub use redact::{Redacted, Secret};
pub use resolver::{Entries, Entry, Resolver};
pub use scope::{AllowedIn, HookCapable, Scope, ScopeKind};
pub use runtime::{Runtime, RuntimeTask, Spawn, TaskEnd, TaskHandle, ValueHandle, spawn_blocking_with, spawn_with};
pub use signal::Signal;
pub use timer::{BoxError, BoxFuture, Bound, Cut, Expiry, RUNTIME_SHUT_DOWN, Refused, RuntimeShutDown, Timer, timeout, timeout_at};
pub use transport::controller::{Controller, ControllerHandle, Handlers, Mount, MountedHandler};
pub use transport::enhancer::EnhancerSpec;
pub use transport::handler::{HandlerInfo, HandlerSpec, Shape};
pub use transport::inputs::Inputs;
pub use transport::metadata::{MetaTier, Metadata, TransportMetadata};
pub use transport::next::Next;
pub use transport::pipeline::{LateOutcome, dispatch, dispatch_late, recover};
pub use transport::server::{BoundAddr, DrainToken, InputReader, Mounted, Server};
pub use transport::{
    AnyErrorHandler, AnyGuard, AnyInterceptor, ErasedErrorHandler, ErasedGuard,
    ErasedInterceptor, ErrorHandler, Guard, Interceptor, Role, Transport,
};
pub use testing::TestApp;
