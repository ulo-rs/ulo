// §12 row: "Two or zero constructors, a bad attribute, a malformed `into` item" (compile), an
// `into` item keyed by a word that is not `value` or `with`. Capability 46.
// Refused at: the key, whose text names the forms an `into` entry takes.
use ulo::module;

trait Plugin: Send + Sync {}

struct Metrics;

impl Plugin for Metrics {}

#[module(providers = [into dyn Plugin: [instance = Metrics]])]
struct Plugins;

fn main() {}
