// A configured module's `Secret<T>` field, through an alias and inside an `Option`, whose `T`
// is not `Display`, so its text cannot be registered.
// Refused at: each module's attribute, labelled at the field's type; neither field is skipped
// unregistered.
use ulo::{Secret, module};

#[derive(Clone, PartialEq, Eq, Hash)]
struct Token([u8; 4]);

type Key = Secret<Token>;

#[derive(Clone, PartialEq, Eq, Hash)]
#[module]
struct Vault {
    key: Key,
}

#[derive(Clone, PartialEq, Eq, Hash)]
#[module]
struct Spare {
    spare: Option<Secret<Token>>,
}

fn main() {}
