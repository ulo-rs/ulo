// The markers `#[handlers]` reads, under a qualifier other than `ulo`: a typo, a path two
// segments deep, and a renamed dependency, on the impl, on a method and inside `cfg_attr`.
// Decision 27 (F595).
// Refused at: each marker's path, with "write the marker bare, or as `ulo::<name>`".
extern crate ulo as framework;

use ulo::{BoxError, Guard, handlers};
use ulo_http::{Http, HttpCx};

struct Allow;

impl Guard<Http> for Allow {
    async fn can_activate(&self, _cx: &HttpCx) -> Result<bool, BoxError> {
        Ok(true)
    }
}

struct Checkout;

#[handlers]
#[ulo::controller]
#[wrong::guards(value = Allow)]
#[ulo::meta(1u8)]
impl Checkout {
    fn new() -> Self {
        Checkout
    }

    #[ulo_http::get("/checkout")]
    #[a::b::interceptors()]
    #[framework::error_handlers()]
    #[wrng::meta(2u8)]
    #[cfg_attr(all(), framework::guards(value = Allow))]
    fn get(&self) -> &'static str {
        "checkout"
    }
}

fn main() {}
