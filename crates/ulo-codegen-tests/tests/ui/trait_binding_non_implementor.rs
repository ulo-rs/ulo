// §12 row: "Trait binding with a non-implementing type" (compile). Capability 46.
// Refused at: the provider entry, where the `also_as` coercion closure needs the trait.
use ulo::{injectable, module};

trait Repo: Send + Sync {}

#[injectable]
struct Memory;

#[module(providers = [Memory as dyn Repo])]
struct Storage;

fn main() {}
