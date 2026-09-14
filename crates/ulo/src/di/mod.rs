//! The dependency-injection system: what a module declares, what resolves it, and the execution a
//! resolution happens in.
//!
//! A module is a DI scope. [`ModuleMetadata`] is what one declares, [`ModuleRef`] is the handle
//! that resolves against it, and [`Execution`] is the one a scoped provider is built for — which
//! transport is running, or none. What an integration crate implements to *be* a provider is
//! `spi`.

mod execution;
mod execution_cache;
mod module_metadata;
mod token;

pub use execution::Execution;
pub use execution_cache::ExecutionCache;
pub use module_metadata::{MiddlewareConsumer, ModuleMetadata};
pub use token::{APP_GUARD, APP_INTERCEPTOR, APP_MIDDLEWARE, IntoToken, Token, token_of};

pub use crate::error::{InitResult, ResolutionError};
pub use crate::extension::{Extension, ExtensionFactory};
pub use crate::injector::ModuleRef;
pub use crate::modules::{CheckedModule, DynamicModule, ModuleIdentity};
pub use crate::provider_scope::ProviderScope;
