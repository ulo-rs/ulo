// A `providers` closure binding followed by `@ Q`.
// Refused at: the `@`, "a closure binding takes no `@` qualifier", naming the `ModuleDef` call.
use ulo::module;

struct Primary;

struct Db(&'static str);

#[module(providers = [with = || Db("primary") @ Primary])]
struct Data;

fn main() {}
