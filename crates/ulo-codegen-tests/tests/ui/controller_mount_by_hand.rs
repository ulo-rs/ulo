// A hand-written `Controller` impl that declares a `mount`. `Controller` is a mark with no
// methods, so a controller cannot mount something other than its `Handlers`. Decision 28.
// Refused at: the `mount` item, as no member of `Controller`.
use ulo::scope::Auto;
use ulo::{Construct, ConstructError, Controller, Dependencies, Mount, Resolver};

struct Hand;

impl Construct for Hand {
    type Scope = Auto;

    fn dependencies(d: &mut Dependencies) {
        d.mark::<Self>();
    }

    async fn construct(_: &Resolver<'_>) -> Result<Self, ConstructError> {
        Ok(Hand)
    }
}

impl Controller for Hand {
    fn mount(_: &mut Mount<'_>) {}
}

fn main() {}
