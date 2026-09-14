//! What a module is: what one declares, the two builders that produce one, and the identity that
//! addresses it.

mod checked;
mod dynamic;
mod identity;
mod metadata;

pub use checked::CheckedModule;
pub use dynamic::DynamicModule;
pub use identity::ModuleIdentity;
pub use metadata::{MiddlewareConsumer, ModuleMetadata};
