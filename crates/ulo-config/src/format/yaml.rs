//! YAML 1.2 through `saphyr`'s `MarkedYaml`.
//!
//! `saphyr`'s loader clones the anchored node at each alias with no bound, so [`prescan`] reads
//! `saphyr-parser`'s event stream first and refuses the file once the nodes the loader would build
//! pass [`ALIAS_LIMIT`]. If the events cannot supply the count, every alias is refused naming its
//! line, as a merge key is.

use std::sync::Arc;

use crate::problems::Problems;
use crate::tree::Tree;

/// The most nodes a file may expand to. Fixed, with no setting: far beyond any configuration file
/// and far below a memory problem.
pub(crate) const ALIAS_LIMIT: usize = 100_000;

pub(crate) fn read(text: &str, file: &Arc<str>) -> Result<Tree, Problems> {
    let _ = (text, file);
    todo!()
}

/// Counts the nodes inside each anchored node as its events arrive, adds an anchor's count at
/// every alias, and records a problem naming the line of the alias that passes [`ALIAS_LIMIT`].
pub(crate) fn prescan(text: &str, file: &Arc<str>) -> Result<(), Problems> {
    let _ = (text, file);
    todo!()
}
