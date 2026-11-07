// `Dependencies::mark` naming a type that is not a controller. Decision 28.
// Refused at: the call, naming the type as not a controller.
use ulo::scope::Auto;
use ulo::{Construct, ConstructError, Dependencies, Resolver};

struct Ledger;

impl Construct for Ledger {
    type Scope = Auto;

    fn dependencies(d: &mut Dependencies) {
        d.mark::<Self>();
    }

    async fn construct(_: &Resolver<'_>) -> Result<Self, ConstructError> {
        Ok(Ledger)
    }
}

fn main() {}
