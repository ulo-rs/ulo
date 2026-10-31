//! Layered configuration (integrations DESIGN §2).
//!
//! The application builds a [`Config`] in `main` from sources it names, each overriding the ones
//! before it, and hands it to the modules that read it. [`ConfigBuilder::load`] never fails: a source
//! that cannot be read or parsed is recorded inside the value and reported by `wire()`, together
//! with every missing, ill-typed or invalid value, one problem per line with its path and origin.
//!
//! ```
//! # use ulo_config::{Config, Defaults, DotEnv, Env, File, Values};
//! # #[derive(Default, serde::Serialize)]
//! # struct AppConfig {}
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
//! serde's derive stops at the first missing field in declaration order, and the leaf types that
//! fail here also refuse a placeholder, so three cases follow:
//!
//! - A top-level section written under a `#[serde(alias)]` key that ends on a leaf-type error is
//!   also reported as `missing required field` under the field's real name.
//! - A missing section whose type refuses a placeholder hides the missing fields declared after
//!   it: with `database` and `cache` both missing, one is reported.
//! - The fields of a `#[serde(flatten)]` struct after a failed section are not reported.
//!
//! ```
//! # use serde::Deserialize;
//! # use ulo_config::{Config, Section};
//! # #[derive(Deserialize)]
//! # struct Url(#[allow(dead_code)] std::net::IpAddr);
//! # #[derive(Deserialize)]
//! # struct Database { #[allow(dead_code)] host: Url }
//! # #[derive(Deserialize)]
//! # struct Cache { #[allow(dead_code)] host: Url }
//! #[derive(Deserialize)]
//! struct App { #[allow(dead_code)] database: Database, #[allow(dead_code)] cache: Cache }
//!
//! let config = Config::builder().load();
//! let section = Section::<App>::read(&config, "");
//! let error = section.value().err().expect("both sections are missing");
//! assert_eq!(error.problems().len(), 1);
//! ```
//!
//! # A report the pass cap cut
//!
//! At most 2048 sections and missing fields are reported together. A read with more ends its
//! report with one final problem, for which [`Problem::truncated`] is `true`: an empty path, no
//! origin, and the message `the report stopped after N problems; fix these and read again`, N being
//! the number of problems before it. A report that holds everything has no such problem.

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
