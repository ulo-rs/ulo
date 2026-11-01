use std::fmt;
use std::sync::Arc;

/// Where a value came from, carried by every leaf of the tree.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Origin {
    /// A file, with the line and column of the value. A `.env` file reports the line alone, and a
    /// failure of the file as a whole carries neither: a zero line or column is "not known" and is
    /// left out of the rendering.
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
            Origin::File { path, line: 0, .. } => f.write_str(path),
            Origin::File { path, line, column: 0 } => write!(f, "{path}:{line}"),
            Origin::File { path, line, column } => write!(f, "{path}:{line}:{column}"),
            Origin::Variable { name } => write!(f, "variable {name}"),
            Origin::Explicit { key } => write!(f, "value set for {key}"),
            Origin::Default => f.write_str("default"),
        }
    }
}

#[derive(Clone)]
pub(crate) struct Node {
    pub(crate) value: Value,
    pub(crate) origin: Origin,
}

#[derive(Clone)]
pub(crate) enum Value {
    Null,
    Bool(bool),
    /// Wide enough for every `u64` and `i64` a file can hold.
    Integer(i128),
    Float(f64),
    String(String),
    /// A string whose type the source does not know: an environment variable or a `.env` value,
    /// which a typed read parses as the number or boolean the field asks for.
    Text(String),
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
        let Some(upper) = upper.root else { return };
        match &mut self.root {
            Some(base) => merge(base, upper),
            None => self.root = Some(upper),
        }
    }

    /// Writes `node` at the path `segments` names, making the tables on the way. A leaf already on
    /// the way is replaced by a table.
    pub(crate) fn set(&mut self, segments: &[String], node: Node) {
        if segments.is_empty() {
            return;
        }
        let root = self.root.get_or_insert_with(|| Node { value: Value::Table(Vec::new()), origin: node.origin.clone() });
        if !matches!(root.value, Value::Table(_)) {
            root.value = Value::Table(Vec::new());
        }
        set_in(root, segments, node);
    }

    /// The node at a dotted path, with `[n]` after a name indexing an array: `http.hosts[1]`. The
    /// empty path is the root.
    pub(crate) fn get(&self, path: &str) -> Option<&Node> {
        let mut node = self.root.as_ref()?;
        if path.is_empty() {
            return Some(node);
        }
        for segment in path.split('.') {
            let (name, mut indexes) = match segment.find('[') {
                Some(at) => segment.split_at(at),
                None => (segment, ""),
            };
            if !name.is_empty() {
                node = match &node.value {
                    Value::Table(entries) => entries.iter().find(|(key, _)| key == name).map(|(_, child)| child)?,
                    _ => return None,
                };
            }
            while let Some(rest) = indexes.strip_prefix('[') {
                let end = rest.find(']')?;
                let index: usize = rest[..end].parse().ok()?;
                node = match &node.value {
                    Value::Array(items) => items.get(index)?,
                    _ => return None,
                };
                indexes = &rest[end + 1..];
            }
            if !indexes.is_empty() {
                return None;
            }
        }
        Some(node)
    }

    /// Every leaf with its dotted path, for the unread-key report.
    pub(crate) fn leaves(&self) -> Vec<(String, &Node)> {
        let mut out = Vec::new();
        if let Some(root) = &self.root {
            collect(root, String::new(), &mut out);
        }
        out
    }
}

impl fmt::Debug for Tree {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Tree").finish_non_exhaustive()
    }
}

/// The path of the entry `key` of the table at `parent`.
pub(crate) fn join_key(parent: &str, key: &str) -> String {
    if parent.is_empty() { key.to_owned() } else { format!("{parent}.{key}") }
}

/// The path of element `index` of the array at `parent`.
pub(crate) fn join_index(parent: &str, index: usize) -> String {
    format!("{parent}[{index}]")
}

fn merge(base: &mut Node, upper: Node) {
    let Node { value, origin } = upper;
    let value = match (&mut base.value, value) {
        (Value::Table(base_entries), Value::Table(upper_entries)) => {
            for (key, node) in upper_entries {
                match base_entries.iter_mut().find(|(existing, _)| *existing == key) {
                    Some((_, existing)) => merge(existing, node),
                    None => base_entries.push((key, node)),
                }
            }
            return;
        }
        (_, value) => value,
    };
    *base = Node { value, origin };
}

fn set_in(table: &mut Node, segments: &[String], node: Node) {
    let Value::Table(entries) = &mut table.value else { return };
    let Some((first, rest)) = segments.split_first() else { return };
    let position = entries.iter().position(|(key, _)| key == first);
    if rest.is_empty() {
        match position {
            Some(at) => entries[at].1 = node,
            None => entries.push((first.clone(), node)),
        }
        return;
    }
    let at = match position {
        Some(at) => at,
        None => {
            entries.push((first.clone(), Node { value: Value::Table(Vec::new()), origin: node.origin.clone() }));
            entries.len() - 1
        }
    };
    let child = &mut entries[at].1;
    if !matches!(child.value, Value::Table(_)) {
        *child = Node { value: Value::Table(Vec::new()), origin: node.origin.clone() };
    }
    set_in(child, rest, node);
}

fn collect<'t>(node: &'t Node, path: String, out: &mut Vec<(String, &'t Node)>) {
    match &node.value {
        Value::Table(entries) => {
            for (key, child) in entries {
                collect(child, join_key(&path, key), out);
            }
        }
        Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                collect(child, join_index(&path, index), out);
            }
        }
        _ => out.push((path, node)),
    }
}
