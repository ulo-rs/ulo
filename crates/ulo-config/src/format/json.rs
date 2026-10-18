//! Strict JSON through `jsonc-parser`'s AST, with comments, trailing commas and loose property
//! names refused. The AST gives a byte range per value; line and column come from counting
//! newlines.

use std::sync::Arc;

use crate::problems::Problems;
use crate::tree::Tree;

pub(crate) fn read(text: &str, file: &Arc<str>) -> Result<Tree, Problems> {
    let _ = (text, file);
    todo!()
}
