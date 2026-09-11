// The inline-struct form was removed. The attribute goes on the struct and
// `#[routes]` on its impl, which is what the diagnostic says to write.
use ulo::controller;

#[controller("/p", pub struct Inline {})]
pub struct Thing {}

fn main() {}
