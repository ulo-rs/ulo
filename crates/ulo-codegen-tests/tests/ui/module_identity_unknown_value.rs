// `identity` takes `type` and nothing else.
// Refused at: the value, "`identity` takes `type`".
use ulo::module;

#[module(identity = value)]
struct Data {
    n: u8,
}

fn main() {}
