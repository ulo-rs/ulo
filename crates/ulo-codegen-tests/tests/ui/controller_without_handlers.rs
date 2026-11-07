// A `#[controller]` type with no `#[handlers]` impl and no `Handlers` impl written by hand.
// Decision 28.
// Refused at: the `#[controller]` attribute, naming the missing `#[handlers]` impl.
use ulo::controller;

#[controller]
struct Checkout;

fn main() {}
