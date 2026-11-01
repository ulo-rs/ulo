// A `providers` entry `expr @ Q` whose `Q` names no type.
// Refused at: the qualifier, rustc's own "cannot find type" spanned on it.
use ulo::module;

struct Db(&'static str);

#[module(providers = [Db("primary") @ Primary])]
struct Data;

fn main() {}
