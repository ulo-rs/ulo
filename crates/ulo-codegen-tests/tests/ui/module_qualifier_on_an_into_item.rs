// An `into K: [..]` item followed by `@ Q`.
// Refused at: the `@`, "an `into` item takes no `@` qualifier", naming the `ModuleDef` call.
use ulo::module;

trait Plugin: Send + Sync {}

struct Primary;

struct Metrics;

impl Plugin for Metrics {}

#[module(providers = [into dyn Plugin: [value = Metrics @ Primary]])]
struct Plugins;

fn main() {}
