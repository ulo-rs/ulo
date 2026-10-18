use std::collections::BTreeSet;
use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};

use crate::source::{Source, SourceFailure};
use crate::tree::{Origin, Tree};

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
    /// Whether a `Defaults` source was added. A typed read is bounded by `DeserializeOwned` alone,
    /// so it cannot call `Default`; the type's `#[serde(default)]` supplies what is missing.
    #[allow(dead_code)]
    pub(crate) defaults: bool,
    /// The paths of the leaves a section read consumed, shared by every clone of the `Config`.
    consumed: Mutex<BTreeSet<String>>,
}

impl Config {
    pub fn builder() -> ConfigBuilder {
        ConfigBuilder { sources: Vec::new() }
    }

    pub(crate) fn mark_consumed(&self, paths: BTreeSet<String>) {
        let mut consumed = self.loaded.consumed.lock().unwrap_or_else(PoisonError::into_inner);
        consumed.extend(paths);
    }

    /// Every leaf supplied by a source that no section read has consumed so far.
    pub(crate) fn unread(&self) -> Vec<(String, Origin)> {
        let consumed = self.loaded.consumed.lock().unwrap_or_else(PoisonError::into_inner);
        self.loaded
            .tree
            .leaves()
            .into_iter()
            .filter(|(path, _)| !consumed.contains(path))
            .map(|(path, node)| (path, node.origin.clone()))
            .collect()
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
        let mut tree = Tree::empty();
        let mut failures = Vec::new();
        let mut defaults = false;
        for source in &self.sources {
            let layer = source.read();
            tree.overlay(layer.tree);
            failures.extend(layer.failures);
            defaults |= layer.defaults;
        }
        Config { loaded: Arc::new(Loaded { tree, failures, defaults, consumed: Mutex::new(BTreeSet::new()) }) }
    }
}
