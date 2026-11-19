//! A wrapper extractor named with a lifetime-carrying payload, awaited inside a `'static` boxed
//! `Send` future, as a user's own extractor or a hand-written handler awaits one. Compiling this
//! file is the assertion: a wrapper whose `from_call` is an `async fn` fails it with
//! "implementation of `Deserialize` is not general enough", or, for `Injected<Dep<dyn Timer>>`,
//! "implementation of `Send` is not general enough" (transports DESIGN decision 107).

use std::borrow::Cow;
use std::future::Future;
use std::pin::Pin;

use ulo::{Dep, Timer};
use ulo_http::{FieldViolation, FromCall, Http, HttpCx, Json, Valid, Validate};
use ulo_transport::Injected;

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

fn optional_json(cx: HttpCx) -> Boxed {
    Box::pin(async move { <Option<Json<Name<'static>>> as FromCall<Http>>::from_call(&cx).await.is_ok() })
}

fn injected_timer(cx: HttpCx) -> Boxed {
    Box::pin(async move { <Injected<Dep<dyn Timer>> as FromCall<Http>>::from_call(&cx).await.is_ok() })
}

#[test]
fn a_valid_wrapper_over_a_lifetime_carrying_payload_is_send() {
    let _: fn(HttpCx) -> Boxed = valid_json;
}

#[test]
fn an_optional_wrapper_over_a_lifetime_carrying_payload_is_send() {
    let _: fn(HttpCx) -> Boxed = optional_json;
}

#[test]
fn an_injected_wrapper_over_a_static_trait_object_is_send() {
    let _: fn(HttpCx) -> Boxed = injected_timer;
}
