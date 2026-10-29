//! Layered configuration (integrations DESIGN §2).
//!
//! The application builds a [`Config`] in `main` from sources it names, each overriding the ones
//! before it, and hands it to the modules that read it. [`ConfigBuilder::load`] never fails: a source
//! that cannot be read or parsed is recorded inside the value and reported by `wire()`, together
//! with every missing, ill-typed or invalid value, one problem per line with its path and origin.
//!
//! ```ignore
//! let config = Config::builder()
//!     .source(Defaults::of::<AppConfig>())
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
//! `untagged` a `Secret` is still registered, and no path is known to be a secret's, so a message
//! its inner type raises is checked afterwards against the text of every secret the section
//! collected: a message containing one is replaced by `invalid value`. That holds only for a
//! secret whose inner type read its input as a scalar before it failed. The same pass reads the
//! messages of failed sources, which a parser may write with the offending line quoted. A secret
//! shorter than four characters is registered but matched against no message, these or the
//! graph's: a text that short is contained in most of them.
//!
//! Under `flatten` serde gives an error in the buffered content no key. Such a problem is reported
//! at the path and origin of the struct that holds the `flatten`, never at a sibling field's, and
//! [`Problem::flattened`] is `true` for it; its rendering ends with `(inside a flattened
//! section)`.
//!
//! # What a report holds
//!
//! The top-level keys of the type being read are sections, and each is read on its own. Every
//! problem in every section is reported. Within a section the report holds the problems found up
//! to the first error raised by a leaf type's own `Deserialize` (an `IpAddr` or a `Url` refusing a
//! string): serde stops there, and nothing outside serde can resume it. A missing field is one
//! problem, however the field's type reads its input.
//!
//! A section that ended on a leaf-type error cannot be given a value, nor can a missing section
//! whose type refuses the placeholder of a field (an `IpAddr` or a `Url` refuses an empty string).
//! The type being read stops at that section, so what it checks after it is not reported in the
//! same load: a missing field declared after it, and the fields of a `#[serde(flatten)]` struct.
//! A section written under a `#[serde(alias)]` key that ends on a leaf-type error is also reported
//! missing under its field's name.

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
pub use source::{Defaults, DotEnv, Env, File, Layer, Scalar, Source, TypeDefaults, Values};
pub use tree::Origin;
