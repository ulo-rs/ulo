// A `providers` entry with two `@` qualifiers.
// Refused at: the second `@`, "an entry takes one `@` qualifier".
use ulo::module;

struct Primary;
struct Replica;

struct Db(&'static str);

#[module(providers = [Db("primary") @ Primary @ Replica])]
struct Data;

fn main() {}
