use ulo::module;

#[module]
#[derive(Clone, PartialEq, Eq, Hash)]
struct Settings(#[cfg(any())] u16, String);

fn main() {}
