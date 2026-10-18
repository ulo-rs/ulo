use std::fmt;
use std::sync::Arc;

/// Where a value came from, carried by every leaf of the tree.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Origin {
    /// A file, with the line and column of the value; a `.env` file reports the line.
    File { path: Arc<str>, line: u32, column: u32 },
    /// An environment variable, by name.
    Variable { name: String },
    /// A value set in code through `Values`, by key.
    Explicit { key: String },
    /// The type's `Default`.
    Default,
}

impl fmt::Display for Origin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Origin::File { path, line, column } => write!(f, "{path}:{line}:{column}"),
            Origin::Variable { name } => write!(f, "variable {name}"),
            Origin::Explicit { key } => write!(f, "value set for {key}"),
            Origin::Default => f.write_str("default"),
        }
    }
}

pub(crate) struct Node {
    pub(crate) value: Value,
    pub(crate) origin: Origin,
}

pub(crate) enum Value {
    Null,
    Bool(bool),
    Integer(i64),
    Float(f64),
    String(String),
    Array(Vec<Node>),
    Table(Vec<(String, Node)>),
}

/// The merged value tree. Its `Debug` prints no value, since a leaf may be a secret.
#[derive(Default)]
pub struct Tree {
    pub(crate) root: Option<Node>,
}

impl Tree {
    pub(crate) fn empty() -> Tree {
        Tree { root: None }
    }

    /// Lays `upper` over `self`: a leaf of `upper` replaces the leaf at its path, tables merge.
    pub(crate) fn overlay(&mut self, upper: Tree) {
        let _ = upper;
        todo!()
    }

    pub(crate) fn get(&self, path: &str) -> Option<&Node> {
        let _ = path;
        todo!()
    }

    /// Every leaf with its dotted path, for the unread-key report.
    pub(crate) fn leaves(&self) -> Vec<(String, &Node)> {
        todo!()
    }
}

impl fmt::Debug for Tree {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Tree").finish_non_exhaustive()
    }
}
