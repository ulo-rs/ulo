//! TOML 1.0 through the `toml` crate's spanned parse; a leaf's line and column come from its byte
//! offset.

use std::sync::Arc;

use crate::problems::Problems;
use crate::tree::Tree;

pub(crate) fn read(text: &str, file: &Arc<str>) -> Result<Tree, Problems> {
    let _ = (text, file);
    todo!()
}
