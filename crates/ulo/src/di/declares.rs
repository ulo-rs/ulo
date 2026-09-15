//! What a type declares itself to be, so a module built at runtime can name the type rather than
//! the factory generated beside it.
//!
//! `#[module(providers: [Db], controllers: [Orders])]` names types, and the macro rewrites each to
//! the factory its attribute generated. A builder method has no macro to do that rewriting, so
//! these traits carry it: `DynamicModule::builder(..).provider_type::<Db>()` reaches the same
//! factory `providers: [Db]` does.

use crate::dispatch::ControllerFactory;
use crate::spi::ProviderFactory;

/// A type `#[injectable]` generated a provider factory for.
///
/// Implemented by the attribute, never by hand — what it answers with is what `providers:` uses.
pub trait DeclaresProvider {
    /// The factory that builds this type, and reports what it must be built after.
    fn provider_factory() -> impl ProviderFactory + 'static;
}

/// A type `#[controller]` generated a controller factory for. See [`DeclaresProvider`].
pub trait DeclaresController {
    /// The factory that builds this target, and reports what it must be built after.
    fn controller_factory() -> impl ControllerFactory + 'static;
}
