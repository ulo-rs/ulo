//! The readers from a file's text into the origin tree.

use std::sync::Arc;

use crate::problems::Problems;
use crate::tree::{Origin, Tree};

pub(crate) mod dotenv;
pub(crate) mod json;
pub(crate) mod toml;
#[cfg(feature = "yaml")]
pub(crate) mod yaml;

#[derive(Clone, Copy, Debug)]
pub(crate) enum Format {
    Toml,
    Json,
    #[cfg(feature = "yaml")]
    Yaml,
}

impl Format {
    /// Reads `text`, the contents of `file`, into a tree whose leaves carry the file, line and
    /// column. Every problem is returned, none dropped after the first.
    pub(crate) fn read(self, text: &str, file: &Arc<str>) -> Result<Tree, Problems> {
        match self {
            Format::Toml => toml::read(text, file),
            Format::Json => json::read(text, file),
            #[cfg(feature = "yaml")]
            Format::Yaml => yaml::read(text, file),
        }
    }
}

/// Byte offsets of line starts, to turn a parser's byte span into the line and column a reader
/// reports.
pub(crate) struct Lines<'t> {
    text: &'t str,
    starts: Vec<usize>,
}

impl<'t> Lines<'t> {
    pub(crate) fn new(text: &'t str) -> Self {
        let mut starts = vec![0];
        starts.extend(text.bytes().enumerate().filter(|(_, byte)| *byte == b'\n').map(|(at, _)| at + 1));
        Lines { text, starts }
    }

    /// The 1-based line and the 1-based column, counted in characters, of a byte offset.
    pub(crate) fn locate(&self, offset: usize) -> (u32, u32) {
        let offset = offset.min(self.text.len());
        let line = match self.starts.binary_search(&offset) {
            Ok(at) => at,
            Err(at) => at.saturating_sub(1),
        };
        let start = self.starts[line];
        let column = self.text.get(start..offset).map_or(offset - start, |before| before.chars().count()) + 1;
        (to_u32(line + 1), to_u32(column))
    }

    pub(crate) fn origin(&self, file: &Arc<str>, offset: usize) -> Origin {
        let (line, column) = self.locate(offset);
        Origin::File { path: file.clone(), line, column }
    }
}

pub(crate) fn to_u32(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// An origin naming the file alone, for a failure that has no position.
pub(crate) fn whole_file(file: &Arc<str>) -> Origin {
    Origin::File { path: file.clone(), line: 0, column: 0 }
}
