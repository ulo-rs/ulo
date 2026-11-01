// An `into K: [..]` list whose `K` is module metadata, `Upgrades`, with a `value =` item.
// Refused at: the key, naming it as metadata and pointing at `m.meta::<K>()`.
use ulo::module;
use ulo_http::Upgrades;

#[module(providers = [into Upgrades: [value = Upgrades::default()]])]
struct Edge;

fn main() {}
