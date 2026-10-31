// An `into K: [..]` list whose `K` is module metadata, `PreDispatch`.
// Refused at: the key, naming it as metadata and pointing at `m.meta::<K>()`.
use ulo::module;
use ulo_http::PreDispatch;

#[module(providers = [into PreDispatch: [value = PreDispatch::default()]])]
struct Edge;

fn main() {}
