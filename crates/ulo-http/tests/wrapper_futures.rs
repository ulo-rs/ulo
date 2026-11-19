//! A wrapper extractor named with a lifetime-carrying payload, awaited inside a `'static` boxed
//! `Send` future, as a user's own extractor or a hand-written handler awaits one. Compiling this
//! file is the assertion: a wrapper whose `from_call` is an `async fn` fails it with
//! "implementation of `Deserialize` is not general enough" (transports DESIGN decision 107).

use std::borrow::Cow;
use std::future::Future;
use std::pin::Pin;

use ulo_http::{FieldViolation, FromCall, Http, HttpCx, Json, Valid, Validate};

type Boxed = Pin<Box<dyn Future<Output = bool> + Send + 'static>>;

#[derive(serde::Deserialize)]
struct Name<'a>(#[allow(dead_code)] Cow<'a, str>);

impl Validate for Name<'_> {
    fn validate(&self) -> Result<(), Vec<FieldViolation>> {
        Ok(())
    }
}

fn valid_json(cx: HttpCx) -> Boxed {
    Box::pin(async move { <Valid<Json<Name<'static>>> as FromCall<Http>>::from_call(&cx).await.is_ok() })
}

#[test]
fn a_valid_wrapper_over_a_lifetime_carrying_payload_is_send() {
    let _: fn(HttpCx) -> Boxed = valid_json;
}
