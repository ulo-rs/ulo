#[cfg(feature = "dev")]
pub mod dev;
#[cfg(feature = "dev")]
pub mod exec;
pub mod generate;
pub mod new;

use anyhow::{Result, bail};

/// Fills the `@@key@@` slots of `template`, and refuses a template that still holds one: the
/// templates are compiled in, so an unfilled slot is a bug here and not something the user wrote,
/// and writing it out would hand them a file that does not build.
pub(crate) fn render(template: &str, values: &[(&str, &str)]) -> Result<String> {
    let mut text = template.to_owned();
    for (key, value) in values {
        text = text.replace(&format!("@@{key}@@"), value);
    }
    if let Some(at) = text.find("@@") {
        let slot: String = text[at..].chars().take_while(|c| !c.is_whitespace()).take(40).collect();
        bail!("the template left the slot `{slot}` unfilled");
    }
    Ok(text)
}

/// The words Rust reserves, as of edition 2024. A name equal to one cannot be a module, a crate or
/// an item.
const KEYWORDS: &[&str] = &[
    "abstract", "as", "async", "await", "become", "box", "break", "const", "continue", "crate", "do", "dyn", "else",
    "enum", "extern", "false", "final", "fn", "for", "gen", "if", "impl", "in", "let", "loop", "macro", "match", "mod",
    "move", "mut", "override", "priv", "pub", "ref", "return", "self", "Self", "static", "struct", "super", "trait",
    "true", "try", "type", "typeof", "unsafe", "unsized", "use", "virtual", "where", "while", "yield",
];

pub(crate) fn is_keyword(word: &str) -> bool {
    KEYWORDS.contains(&word)
}
