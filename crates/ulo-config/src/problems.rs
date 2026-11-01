use std::fmt;

use crate::tree::Origin;

/// One finding about the configuration: a missing, ill-typed or invalid value, a failed source, or
/// a finding of a validation hook.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Problem {
    pub(crate) path: String,
    pub(crate) origin: Option<Origin>,
    pub(crate) message: String,
    pub(crate) flattened: bool,
    pub(crate) truncated: bool,
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

    /// Whether the deserializer could not attribute this problem inside a `#[serde(flatten)]`
    /// section: serde buffers a flattened field and gives its error no key, so the path and origin
    /// are those of the struct that holds the flatten, not of the failing value. A problem is
    /// never reported against a sibling field's origin; this marker says the path and origin name
    /// the containing struct.
    pub fn flattened(&self) -> bool {
        self.flattened
    }

    /// Whether this is the notice that ends a report the read cut short: at most 2048 sections
    /// and missing fields are reported together, and a read with more ends with one problem that
    /// has an empty path, no origin and the message `the report stopped after N problems; fix
    /// these and read again`, N being the number of problems before it. A report that holds
    /// everything has no such problem.
    pub fn truncated(&self) -> bool {
        self.truncated
    }
}

/// Every problem found, in the order found. The order is deterministic: the same failing input
/// gives the same problems in the same order on every load. No particular order is promised, so a
/// caller that needs one sorts.
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
        self.list.push(Problem { path: path.into(), origin: None, message: message.into(), flattened: false, truncated: false });
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
        self.list.push(Problem { path: path.into(), origin: Some(origin), message: message.into(), flattened: false, truncated: false });
    }
}

/// The header every standalone report starts with; a section read or `ConfigModule` supplies its
/// own through `ConfigError`.
const HEADER: &str = "configuration is invalid";

impl fmt::Display for Problems {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_report(f, HEADER, self)
    }
}

/// The header line with the count, then one problem per line. Newlines are the separator because
/// `Redacted` flattens an error into one string, and the renderer indents continuation lines.
/// The count is of real problems: a report cut at the pass cap ends with a notice that is not
/// one, and the header says `truncated` in its place.
pub(crate) fn write_report(f: &mut fmt::Formatter<'_>, header: &str, problems: &Problems) -> fmt::Result {
    let cut = problems.list.iter().any(|p| p.truncated);
    let count = problems.list.len() - usize::from(cut);
    let plural = if count == 1 { "" } else { "s" };
    let tail = if cut { ", truncated" } else { "" };
    write!(f, "{header} ({count} problem{plural}{tail})")?;
    for problem in &problems.list {
        f.write_str("\n")?;
        let flattened = problem.flattened;
        match (problem.path.is_empty(), &problem.origin) {
            (true, Some(origin)) => write!(f, "{origin}: {}", problem.message)?,
            (true, None) => f.write_str(&problem.message)?,
            (false, Some(origin)) => write!(f, "{} at {origin}: {}", problem.path, problem.message)?,
            (false, None) => write!(f, "{}: {}", problem.path, problem.message)?,
        }
        if flattened {
            f.write_str(" (inside a flattened section)")?;
        }
    }
    Ok(())
}

impl std::error::Error for Problems {}
