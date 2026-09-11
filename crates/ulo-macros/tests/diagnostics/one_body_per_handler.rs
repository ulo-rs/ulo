// ADR-0040: a handler reads the body once. The check is per parameter pair off
// `FromContext::CONSUMES`, so the message names both offenders.
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
    async fn x(&self, _raw: Bytes, _typed: Json<Payload>) -> Body {
        Body::text("never reached")
    }
}

fn main() {}
