// `#[derive(Classify)]` on a type with no `Display` and `Error`: one error, the derive's, whose
// note names the thiserror pairing.
use ulo_transport::Classify;

#[derive(Debug, Classify)]
#[classify(not_found)]
pub struct NoteNotFound;

fn main() {}
