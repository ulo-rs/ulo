// A hand-written controller with the `Controller` mark and no `Handlers` impl, registered with
// `m.controller::<C>()`. Decision 28.
// Refused at: the registration, naming the type as not a controller with handlers.
use ulo::scope::Auto;
use ulo::{Construct, ConstructError, Controller, Dependencies, ModuleDef, Resolver};

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

impl Controller for Hand {}

fn register(m: &mut ModuleDef<'_>) {
    m.controller::<Hand>();
}

fn main() {
    let _ = register;
}
