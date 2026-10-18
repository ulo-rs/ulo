use std::fmt;
use std::sync::Arc;

use crate::source::{Source, SourceFailure};
use crate::tree::Tree;

/// The sources merged into one origin-carrying tree, as `main` loaded them.
///
/// A `Config` is a cheap handle: clone it into every module that reads a section. It holds the
/// values of every source, secrets included, and its `Debug` prints none of them.
#[derive(Clone)]
pub struct Config {
    pub(crate) loaded: Arc<Loaded>,
}

pub(crate) struct Loaded {
    pub(crate) tree: Tree,
    pub(crate) failures: Vec<SourceFailure>,
    /// Whether a `Defaults` source was added, which a typed read lays under the file layers.
    pub(crate) defaults: bool,
}

impl Config {
    pub fn builder() -> ConfigBuilder {
        ConfigBuilder { sources: Vec::new() }
    }
}

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config").finish_non_exhaustive()
    }
}

/// Collects sources in the order they layer.
pub struct ConfigBuilder {
    sources: Vec<Box<dyn Source>>,
}

impl ConfigBuilder {
    /// Adds a source that overrides every source added before it.
    pub fn source(mut self, source: impl Source + 'static) -> Self {
        self.sources.push(Box::new(source));
        self
    }

    /// Reads every source and merges them. Never fails: a source that is missing (unless
    /// `.optional()`), unreadable or malformed is recorded in the value, and each module that reads
    /// a section from it reports that failure through `wire()`. Reads files, so it runs in `main`
    /// and not inside a runtime worker.
    pub fn load(self) -> Config {
        todo!()
    }
}
