// §12 row: "Two or zero constructors, a bad attribute, a malformed `into` item" (compile), an
// `into` item that is an expression. Capability 46.
// Refused at: the item, whose text names the three forms: a type, `value = expr`, `with = ..`.
use ulo::module;

trait Plugin: Send + Sync {}

struct Metrics;

impl Plugin for Metrics {}

#[module(providers = [into dyn Plugin: [make_plugin()]])]
struct Plugins;

fn make_plugin() -> Metrics {
    Metrics
}

fn main() {}
