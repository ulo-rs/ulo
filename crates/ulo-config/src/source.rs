use std::collections::BTreeMap;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;

use crate::format::{Format, dotenv};
use crate::problems::Problems;
use crate::tree::{Node, Origin, Tree, Value};

mod sealed {
    pub trait Sealed {}
}

/// One layer of configuration, read by [`ConfigBuilder::load`](crate::ConfigBuilder::load).
///
/// Sealed: the five sources below are the design's, and a remote source is deferred, since a
/// network read before anything serves is not one `load` bounds or retries.
pub trait Source: sealed::Sealed {
    /// Reads the source. Never fails; a failure is recorded in the layer.
    fn read(&self) -> Layer;
}

/// What one source read produced: its values, each with its origin, and the failures of the read.
pub struct Layer {
    pub(crate) tree: Tree,
    pub(crate) failures: Vec<SourceFailure>,
    pub(crate) defaults: bool,
}

pub(crate) struct SourceFailure {
    /// The source as the report names it: a file's path, `.env`, the environment.
    pub(crate) source: String,
    pub(crate) problems: Problems,
}

impl Layer {
    fn of(tree: Tree) -> Layer {
        Layer { tree, failures: Vec::new(), defaults: false }
    }

    fn failed(source: &str, message: impl Into<String>) -> Layer {
        let mut problems = Problems::new();
        problems.at("", message);
        Layer::failed_with(source, problems)
    }

    fn failed_with(source: &str, problems: Problems) -> Layer {
        Layer {
            tree: Tree::empty(),
            failures: vec![SourceFailure { source: source.to_owned(), problems }],
            defaults: false,
        }
    }
}

/// Reads a file's text, or answers the layer that records why it could not: a missing file is no
/// failure when `optional`, in which case the layer is empty.
fn read_text(path: &std::path::Path, optional: bool) -> Result<(Arc<str>, String), Layer> {
    let name = path.to_string_lossy().into_owned();
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(if optional { Layer::of(Tree::empty()) } else { Layer::failed(&name, "file not found") });
        }
        Err(error) => return Err(Layer::failed(&name, format!("file cannot be read: {error}"))),
    };
    match String::from_utf8(bytes) {
        Ok(text) => {
            let text = if text.starts_with('\u{feff}') { text['\u{feff}'.len_utf8()..].to_owned() } else { text };
            Ok((Arc::from(name), text))
        }
        Err(_) => Err(Layer::failed(&name, "file is not valid UTF-8")),
    }
}

/// The path a `.env` key or an environment variable name stands for: the name without `prefix`,
/// split at `separator`, each segment lowercased so `APP_HTTP__PORT` is `http.port`.
fn segments(name: &str, prefix: Option<&str>, separator: &str) -> Option<Vec<String>> {
    let rest = match prefix {
        Some(prefix) => name.strip_prefix(prefix)?,
        None => name,
    };
    let segments: Vec<String> = if separator.is_empty() {
        vec![rest.to_lowercase()]
    } else {
        rest.split(separator).filter(|segment| !segment.is_empty()).map(str::to_lowercase).collect()
    };
    if segments.iter().all(String::is_empty) { None } else { Some(segments) }
}

/// The type's `Default` as the bottom layer. The type is not known when the builder runs, so the
/// layer is a marker that `ConfigModule` and a section read resolve against the type they read.
#[derive(Clone, Copy, Debug, Default)]
pub struct Defaults;

impl sealed::Sealed for Defaults {}

impl Source for Defaults {
    fn read(&self) -> Layer {
        Layer { tree: Tree::empty(), failures: Vec::new(), defaults: true }
    }
}

#[derive(Clone, Copy, Debug)]
enum Kind {
    Toml,
    Json,
    #[cfg(feature = "yaml")]
    Yaml,
}

/// A configuration file, required unless `.optional()`.
#[derive(Clone, Debug)]
pub struct File {
    path: PathBuf,
    kind: Kind,
    optional: bool,
}

impl File {
    /// TOML 1.0, read with the `toml` crate's spans.
    pub fn toml(path: impl Into<PathBuf>) -> Self {
        File { path: path.into(), kind: Kind::Toml, optional: false }
    }

    /// Strict JSON (RFC 8259): no comments, no trailing commas, line and column per value.
    pub fn json(path: impl Into<PathBuf>) -> Self {
        File { path: path.into(), kind: Kind::Json, optional: false }
    }

    /// YAML 1.2 by the core schema. Anchors and aliases are bounded at 100,000 expanded nodes and a
    /// merge key (`<<`) is a problem naming its line.
    #[cfg(feature = "yaml")]
    pub fn yaml(path: impl Into<PathBuf>) -> Self {
        File { path: path.into(), kind: Kind::Yaml, optional: false }
    }

    /// A missing file is no failure. A file that exists and does not parse still is.
    pub fn optional(mut self) -> Self {
        self.optional = true;
        self
    }
}

impl File {
    fn format(&self) -> Format {
        match self.kind {
            Kind::Toml => Format::Toml,
            Kind::Json => Format::Json,
            #[cfg(feature = "yaml")]
            Kind::Yaml => Format::Yaml,
        }
    }
}

impl sealed::Sealed for File {}

impl Source for File {
    fn read(&self) -> Layer {
        let (name, text) = match read_text(&self.path, self.optional) {
            Ok(read) => read,
            Err(layer) => return layer,
        };
        match self.format().read(&text, &name) {
            Ok(tree) => Layer::of(tree),
            Err(problems) => Layer::failed_with(&name, problems),
        }
    }
}

/// A `.env` file in the subset the crate's own parser reads: `KEY=VALUE` with an optional `export `
/// prefix, single quotes literal, `\n`, `\"` and `\\` escapes in double quotes, `#` comments, no
/// expansion and no multi-line values. A key written twice in one file is a problem naming both
/// lines. The file is read into the layer and never into the process environment.
///
/// Its keys map to paths as `Env`'s do, through [`prefix`](Self::prefix) and
/// [`separator`](Self::separator).
#[derive(Clone, Debug)]
pub struct DotEnv {
    path: PathBuf,
    optional: bool,
    prefix: Option<String>,
    separator: String,
}

impl DotEnv {
    pub fn file(path: impl Into<PathBuf>) -> Self {
        DotEnv { path: path.into(), optional: false, prefix: None, separator: String::from("__") }
    }

    pub fn optional(mut self) -> Self {
        self.optional = true;
        self
    }

    /// Only keys starting with `prefix` are read, with the prefix removed.
    pub fn prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefix = Some(prefix.into());
        self
    }

    /// The text in a key that separates the segments of a path, `__` by default.
    pub fn separator(mut self, separator: impl Into<String>) -> Self {
        self.separator = separator.into();
        self
    }
}

impl sealed::Sealed for DotEnv {}

impl Source for DotEnv {
    fn read(&self) -> Layer {
        let (name, text) = match read_text(&self.path, self.optional) {
            Ok(read) => read,
            Err(layer) => return layer,
        };
        let entries = match dotenv::parse(&text, &name) {
            Ok(entries) => entries,
            Err(problems) => return Layer::failed_with(&name, problems),
        };
        let mut tree = Tree::empty();
        for entry in entries {
            let Some(path) = segments(&entry.key, self.prefix.as_deref(), &self.separator) else { continue };
            let origin = Origin::File { path: name.clone(), line: entry.line, column: 0 };
            tree.set(&path, Node { value: Value::Text(entry.value), origin });
        }
        Layer::of(tree)
    }
}

/// Environment variables, read from the process by [`Env::process`] or from a map a test passes.
///
/// A name loses its [`prefix`](Self::prefix), is split at the [`separator`](Self::separator) and is
/// lowercased into a path: `APP_HTTP__PORT` under the prefix `APP_` is `http.port`. A value is
/// text, which a typed read parses as the number or boolean the field asks for. A variable that
/// is not UTF-8 is left out.
#[derive(Clone, Debug)]
pub struct Env {
    vars: Option<BTreeMap<String, String>>,
    prefix: Option<String>,
    separator: String,
}

impl Env {
    /// The process environment, read when the builder loads.
    pub fn process() -> Self {
        Env { vars: None, prefix: None, separator: String::from("__") }
    }

    pub fn from_map<K, V>(vars: impl IntoIterator<Item = (K, V)>) -> Self
    where
        K: Into<String>,
        V: Into<String>,
    {
        let vars = vars.into_iter().map(|(k, v)| (k.into(), v.into())).collect();
        Env { vars: Some(vars), prefix: None, separator: String::from("__") }
    }

    /// Only variables starting with `prefix` are read, with the prefix removed, and a variable
    /// beginning with it that no section reads is warned about at `connect`.
    pub fn prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefix = Some(prefix.into());
        self
    }

    /// The text in a name that separates the segments of a path, `__` by default.
    pub fn separator(mut self, separator: impl Into<String>) -> Self {
        self.separator = separator.into();
        self
    }

    /// One variable by its full name, from the map or the process: how `ulo-otel` reads the
    /// standard `OTEL_*` names from the `Env` it is given.
    pub fn var(&self, name: &str) -> Option<String> {
        match &self.vars {
            Some(vars) => vars.get(name).cloned(),
            None => std::env::var(name).ok(),
        }
    }
}

impl sealed::Sealed for Env {}

impl Source for Env {
    fn read(&self) -> Layer {
        // A name or value that is not UTF-8 cannot be a path or a configuration value, and is left
        // out rather than aborting the read the way `std::env::vars` would.
        let vars: BTreeMap<String, String> = match &self.vars {
            Some(vars) => vars.clone(),
            None => std::env::vars_os()
                .filter_map(|(name, value)| Some((name.into_string().ok()?, value.into_string().ok()?)))
                .collect(),
        };
        let mut tree = Tree::empty();
        for (name, value) in vars {
            let Some(path) = segments(&name, self.prefix.as_deref(), &self.separator) else { continue };
            tree.set(&path, Node { value: Value::Text(value), origin: Origin::Variable { name } });
        }
        Layer::of(tree)
    }
}

/// A value of a type a `Values` entry accepts.
#[derive(Clone, Debug, PartialEq)]
pub enum Scalar {
    Bool(bool),
    Integer(i64),
    Float(f64),
    String(String),
}

macro_rules! scalar_from {
    ($($ty:ty => $variant:ident as $cast:ty),* $(,)?) => {
        $(impl From<$ty> for Scalar {
            fn from(v: $ty) -> Self {
                Scalar::$variant(v as $cast)
            }
        })*
    };
}

scalar_from! {
    i8 => Integer as i64,
    i16 => Integer as i64,
    i32 => Integer as i64,
    i64 => Integer as i64,
    u8 => Integer as i64,
    u16 => Integer as i64,
    u32 => Integer as i64,
    f32 => Float as f64,
    f64 => Float as f64,
}

impl From<bool> for Scalar {
    fn from(v: bool) -> Self {
        Scalar::Bool(v)
    }
}

impl From<&str> for Scalar {
    fn from(v: &str) -> Self {
        Scalar::String(v.to_owned())
    }
}

impl From<String> for Scalar {
    fn from(v: String) -> Self {
        Scalar::String(v)
    }
}

/// Values set in code, each at a dotted path: `Values::new().set("http.port", 8080)`.
#[derive(Clone, Debug, Default)]
pub struct Values {
    entries: Vec<(String, Scalar)>,
}

impl Values {
    pub fn new() -> Self {
        Values { entries: Vec::new() }
    }

    pub fn set(mut self, path: impl Into<String>, value: impl Into<Scalar>) -> Self {
        self.entries.push((path.into(), value.into()));
        self
    }
}

impl sealed::Sealed for Values {}

impl Source for Values {
    fn read(&self) -> Layer {
        let mut tree = Tree::empty();
        for (key, scalar) in &self.entries {
            let path: Vec<String> = key.split('.').filter(|segment| !segment.is_empty()).map(str::to_owned).collect();
            let value = match scalar {
                Scalar::Bool(flag) => Value::Bool(*flag),
                Scalar::Integer(number) => Value::Integer(i128::from(*number)),
                Scalar::Float(number) => Value::Float(*number),
                Scalar::String(text) => Value::String(text.clone()),
            };
            tree.set(&path, Node { value, origin: Origin::Explicit { key: key.clone() } });
        }
        Layer::of(tree)
    }
}
