// `Option<T>` forwards `T::CONSUMES`, so an optional body extractor still
// counts as the one body reader. Without the forwarding this compiles and
// fails at request time instead, which is the whole reason the check is at
// compile time.
use serde::Deserialize;
use ulo::extractors::{Bytes, Json};
use ulo::{Body, controller, post, routes};

#[derive(Deserialize)]
pub struct Payload {
    pub a: i32,
}

#[controller("/p")]
pub struct C {}

#[routes]
impl C {
    #[post("/x")]
    async fn x(&self, _maybe: Option<Json<Payload>>, _raw: Bytes) -> Body {
        Body::text("never reached")
    }
}

fn main() {}
