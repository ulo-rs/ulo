//! Shared helpers for the configuration tests: a scratch directory per test, the merged tree read
//! back as JSON, and the problems of a section read.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use serde::de::DeserializeOwned;
use serde_json::Value;
use ulo_config::{Config, Origin, Problem, Section};

/// A directory of its own, removed on drop.
pub struct Dir(PathBuf);

impl Dir {
    pub fn new() -> Dir {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let path = std::env::temp_dir().join(format!("ulo-config-layers-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::SeqCst)));
        std::fs::create_dir_all(&path).expect("the scratch directory is created");
        Dir(path)
    }

    /// Writes `text` to `name` and answers the file's path.
    pub fn write(&self, name: &str, text: &str) -> PathBuf {
        let path = self.0.join(name);
        std::fs::write(&path, text).expect("the file is written");
        path
    }

    pub fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The whole merged tree as JSON: tables are objects, and a value from the environment or a
/// `.env` file, which the source cannot type, is a string.
pub fn tree(config: &Config) -> Value {
    Section::<Value>::read(config, "").value().cloned().unwrap_or_else(|error| panic!("the tree reads as JSON: {error}"))
}

/// Every problem reading the section `path` as `T` raised, in report order; panics when it reads.
pub fn problems_of<T: DeserializeOwned>(config: &Config, path: &str) -> Vec<Problem> {
    match Section::<T>::read(config, path).value() {
        Ok(_) => panic!("section `{path}` was expected to be refused, and it read"),
        Err(error) => error.problems().iter().cloned().collect(),
    }
}

/// The path, line and column of a file origin; panics on any other origin.
pub fn file_at(origin: &Origin) -> (String, u32, u32) {
    match origin {
        Origin::File { path, line, column } => (path.to_string(), *line, *column),
        other => panic!("expected a file origin, found {other:?}"),
    }
}

pub fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// Path and origin of each problem.
pub fn pairs<'a>(problems: impl Iterator<Item = &'a Problem>) -> Vec<(String, Option<Origin>)> {
    problems.map(|problem| (problem.path().to_owned(), problem.origin().cloned())).collect()
}

/// `pairs` in an order that does not depend on the order problems were found in.
pub fn sorted(mut pairs: Vec<(String, Option<Origin>)>) -> Vec<(String, Option<Origin>)> {
    pairs.sort_by_key(|(path, origin)| (path.clone(), format!("{origin:?}")));
    pairs
}

/// The problems that name a source, leaving out what an empty tree adds beside them: a failed
/// source supplies nothing, so a read of the whole tree also reports a missing value.
pub fn located<T: DeserializeOwned>(config: &Config, path: &str) -> Vec<Problem> {
    problems_of::<T>(config, path).into_iter().filter(|problem| problem.origin().is_some()).collect()
}
