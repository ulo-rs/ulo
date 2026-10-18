//! The readers from a file's text into the origin tree.

use std::sync::Arc;

use crate::problems::Problems;
use crate::tree::Tree;

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
    #[allow(dead_code)]
    pub(crate) fn read(self, text: &str, file: &Arc<str>) -> Result<Tree, Problems> {
        let _ = (text, file);
        todo!()
    }
}
