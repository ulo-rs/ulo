//! The dependency-injection system: what a module declares, what resolves it, and the execution a
//! resolution happens in.
//!
//! A module is a DI scope. [`ModuleMetadata`] is what one declares, [`ModuleRef`] is the handle
//! that resolves against it, and [`Execution`] is the one a scoped provider is built for — which
//! transport is running, or none. What an integration crate implements to *be* a provider is
//! `spi`.

mod declares;
mod execution;
mod execution_cache;
mod extension;
pub(crate) mod internal;
pub(crate) mod module;
mod scope;
mod token;

pub use declares::{DeclaresController, DeclaresProvider};
pub use execution::Execution;
pub use execution_cache::ExecutionCache;
pub use extension::{Extension, ExtensionFactory};
pub use internal::ModuleRef;
pub use module::{
    CheckedModule, DynamicModule, MiddlewareConsumer, ModuleIdentity, ModuleMetadata,
};
pub use scope::ProviderScope;
pub use token::{APP_GUARD, APP_INTERCEPTOR, APP_MIDDLEWARE, IntoToken, Token, token_of};

pub use crate::error::{InitResult, ResolutionError};
