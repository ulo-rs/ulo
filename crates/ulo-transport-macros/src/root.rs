//! The path the derives' output names the transport-neutral items through.

use std::collections::HashSet;

use proc_macro2::{Span, TokenStream};
use proc_macro_crate::{FoundCrate, crate_name};
use quote::quote;
use syn::Ident;

/// The crates that export `Classify`, `Validate`, `ErrorKind`, `FieldViolation` and the
/// doc-hidden `__derive` module the generated code calls, in the order the user's manifest is
/// searched.
const CANDIDATES: &[&str] = &["ulo-transport", "ulo-rpc", "ulo-http", "ulo-ws", "ulo-grpc"];

/// The first of [`CANDIDATES`] the deriving crate depends on, as an absolute path, so an
/// application that depends on `ulo-http` alone derives through it. A renamed dependency is
/// found under its new name. A candidate under `[dependencies]` wins over one under
/// `[dev-dependencies]`, which a library or binary target cannot name. With none of them present
/// the path names `ulo_transport`, whose absence rustc then reports at the derive.
pub(crate) fn transport() -> TokenStream {
    let found: Vec<(&str, String)> = CANDIDATES
        .iter()
        .filter_map(|candidate| match crate_name(candidate) {
            // A derive used inside the crate that defines it: its own tests and doctests link it
            // under its library name.
            Ok(FoundCrate::Itself) => Some((*candidate, candidate.replace('-', "_"))),
            Ok(FoundCrate::Name(name)) => Some((*candidate, name)),
            Err(_) => None,
        })
        .collect();
    let normal = normal_dependencies();
    let chosen = found.iter().find(|(candidate, _)| normal.contains(*candidate)).or(found.first());
    let name = chosen.map_or("ulo_transport", |(_, name)| name.as_str());
    let ident = Ident::new(name, Span::call_site());
    quote!(::#ident)
}

/// The package names under `[dependencies]` and `[target.'..'.dependencies]` of the deriving
/// crate's manifest, a renamed entry under its `package`. Empty when the manifest cannot be read,
/// which leaves the first found candidate chosen.
fn normal_dependencies() -> HashSet<String> {
    let Some(dir) = std::env::var_os("CARGO_MANIFEST_DIR") else { return HashSet::new() };
    let Ok(text) = std::fs::read_to_string(std::path::Path::new(&dir).join("Cargo.toml")) else { return HashSet::new() };
    let Ok(doc) = text.parse::<toml_edit::DocumentMut>() else { return HashSet::new() };
    let targets = doc.get("target").and_then(|t| t.as_table_like()).into_iter().flat_map(|t| t.iter().map(|(_, v)| v));
    let tables = doc.get("dependencies").into_iter().chain(targets.filter_map(|t| t.get("dependencies")));
    tables
        .filter_map(|t| t.as_table_like())
        .flat_map(|t| t.iter().map(|(key, value)| value.get("package").and_then(|p| p.as_str()).unwrap_or(key).to_owned()).collect::<Vec<_>>())
        .collect()
}
