use std::fmt;

use crate::tree::Origin;

/// One finding about the configuration: a missing, ill-typed or invalid value, a failed source, or
/// a finding of a validation hook.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Problem {
    pub(crate) path: String,
    pub(crate) origin: Option<Origin>,
    pub(crate) message: String,
}

impl Problem {
    /// The dotted path of the value, `database.url`.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Where the value came from, when the problem is about a value that was supplied.
    pub fn origin(&self) -> Option<&Origin> {
        self.origin.as_ref()
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

/// Every problem found, in the order found.
///
/// A validation hook, `Store::validate` and a section read all report through it, so one `wire()`
/// error lists them together. The `Display` is a header line and one problem per line, each with
/// its path and origin. A problem under a path recorded as a secret's renders from path and kind
/// alone.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Problems {
    pub(crate) list: Vec<Problem>,
}

impl Problems {
    pub fn new() -> Self {
        Problems { list: Vec::new() }
    }

    /// Records a problem at `path`. Inside a section the path is relative to the section, and the
    /// section read prefixes it.
    pub fn at(&mut self, path: impl Into<String>, message: impl Into<String>) {
        self.list.push(Problem { path: path.into(), origin: None, message: message.into() });
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    pub fn len(&self) -> usize {
        self.list.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Problem> {
        self.list.iter()
    }

    /// `Ok` when nothing was recorded.
    pub fn into_result(self) -> Result<(), Problems> {
        if self.list.is_empty() { Ok(()) } else { Err(self) }
    }

    pub(crate) fn at_origin(&mut self, path: impl Into<String>, origin: Origin, message: impl Into<String>) {
        self.list.push(Problem { path: path.into(), origin: Some(origin), message: message.into() });
    }
}

impl fmt::Display for Problems {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let _ = f;
        todo!()
    }
}

impl std::error::Error for Problems {}
