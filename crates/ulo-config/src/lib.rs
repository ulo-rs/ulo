//! Layered configuration (integrations DESIGN §2).
//!
//! The application builds a [`Config`] in `main` from sources it names, each overriding the ones
//! before it, and hands it to the modules that read it. [`Config::load`] never fails: a source
//! that cannot be read or parsed is recorded inside the value and reported by `wire()`, together
//! with every missing, ill-typed or invalid value, one problem per line with its path and origin.
//!
//! ```ignore
//! let config = Config::builder()
//!     .source(Defaults)
//!     .source(File::toml("config/app.toml"))
//!     .source(DotEnv::file(".env").optional())
//!     .source(Env::process().prefix("APP_").separator("__"))
//!     .source(Values::new().set("http.port", 8080))
//!     .load();
//! ```
//!
//! No source is found on its own: the process environment is read only by `Env::process()`, and a
//! `.env` file is read into the layer and never into the process environment.
//!
//! # What an error prints
//!
//! A type or value problem is rendered from the kind of the value found and never the value, under
//! every path, buffered content behind `#[serde(flatten)]` and `#[serde(untagged)]` included. A
//! leaf type's own `Error::custom` message is printed as that type wrote it, except under the path
//! of a [`Secret`](ulo::Secret), where only the path and kind are printed. Under `flatten` or
//! `untagged` a `Secret` is still registered, but an error its inner type raises through
//! `Error::custom` is printed as written, since a value that failed was never collected.
//!
//! A section reports at most one leaf-type error: serde stops a struct's pass at the first error
//! raised inside a leaf type's own `Deserialize`, and every other section still reports its own.

mod config;
mod de;
mod error;
mod format;
mod module;
mod problems;
mod section;
mod source;
mod tree;

pub use config::{Config, ConfigBuilder};
pub use error::ConfigError;
pub use module::ConfigModule;
pub use problems::{Problem, Problems};
pub use section::Section;
pub use source::{Defaults, DotEnv, Env, File, Layer, Scalar, Source, Values};
pub use tree::Origin;
