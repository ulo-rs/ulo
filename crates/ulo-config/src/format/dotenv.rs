//! The `.env` subset (integrations DESIGN §2.1): `KEY=VALUE` with an optional `export ` prefix,
//! single quotes literal, `\n`, `\"` and `\\` in double quotes, `#` at the start of a line or after
//! whitespace in an unquoted value, no expansion and no multi-line values.

use std::sync::Arc;

use crate::problems::Problems;

/// One `KEY=VALUE` line, with the line number that becomes the value's origin.
pub(crate) struct Entry {
    pub(crate) key: String,
    pub(crate) value: String,
    pub(crate) line: u32,
}

/// Every entry of `text`, or every problem: a malformed line, and a key written twice, which names
/// both lines.
pub(crate) fn parse(text: &str, file: &Arc<str>) -> Result<Vec<Entry>, Problems> {
    let _ = (text, file);
    todo!()
}
