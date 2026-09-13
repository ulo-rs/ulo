// ADR-0012: `{param}` is the only parameter syntax. An Express-style `:param`
// is refused at the macro, naming the segment and the spelling to use.
use ulo::{controller, get, routes};
use ulo::http::Body;
#[controller("/users")]
pub struct UsersController {}

#[routes]
impl UsersController {
    #[get("/:id")]
    fn get_user(&self) -> Body {
        Body::text("never reached")
    }
}

fn main() {}
