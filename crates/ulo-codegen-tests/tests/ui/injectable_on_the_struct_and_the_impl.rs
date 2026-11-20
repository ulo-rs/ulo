// A type declared a provider in both forms: `#[injectable]` on the struct, which builds it from its
// fields, and on its impl, whose constructor builds it. Capability 46.
// Refused at: the second `#[injectable]`, E0119 on the impls both forms write.
use ulo::{Dep, injectable};

struct Config;

#[injectable]
struct Orders {
    config: Dep<Config>,
}

#[injectable]
impl Orders {
    fn new(config: Dep<Config>) -> Self {
        Orders { config }
    }
}

fn main() {}
