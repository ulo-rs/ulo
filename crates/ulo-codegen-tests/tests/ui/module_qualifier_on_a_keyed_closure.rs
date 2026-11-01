// A `K: with = ..` closure binding followed by `@ Q`.
// Refused at: the `@`, "a closure binding takes no `@` qualifier", naming the `ModuleDef` call.
use ulo::module;

trait Sink: Send + Sync {}

struct Primary;

struct Drain;

impl Sink for Drain {}

#[module(providers = [dyn Sink: with = || Drain @ Primary])]
struct Sinks;

fn main() {}
