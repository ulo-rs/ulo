//! A guard refuses two ways and the status says which: `Ok(false)` is an authenticated caller
//! who may not (403), `Err(CallError::unauthorized(..))` is a caller who has not proved who they
//! are (401, carrying the challenge).

mod o1_support;

use http::StatusCode;
use o1_support::{Call, start};
use ulo::{BoxError, Guard, ModuleDef, injectable, routes};
use ulo_http::{Http, HttpCx};
use ulo_transport::CallError;

struct Forbids;

impl Guard<Http> for Forbids {
    async fn can_activate(&self, _cx: &HttpCx) -> Result<bool, BoxError> {
        Ok(false)
    }
}

struct Challenges;

impl Guard<Http> for Challenges {
    async fn can_activate(&self, _cx: &HttpCx) -> Result<bool, BoxError> {
        Err(CallError::unauthorized("Basic realm=\"vault\"").into())
    }
}

#[injectable]
struct Vault;

#[routes]
impl Vault {
    #[ulo_http::get("/forbidden")]
    #[guards(value = Forbids)]
    fn forbidden(&self) -> &'static str {
        "unreachable"
    }

    #[ulo_http::get("/unauthenticated")]
    #[guards(value = Challenges)]
    fn unauthenticated(&self) -> &'static str {
        "unreachable"
    }
}

fn app(m: &mut ModuleDef<'_>) {
    m.controller::<Vault>();
}

#[tokio::test]
async fn a_guard_answering_false_is_a_403_and_one_failing_unauthorized_is_a_401_with_its_challenge() {
    let app = start(app).await;

    let forbidden = app.send(Call::get("/forbidden")).await;
    assert_eq!(forbidden.status, StatusCode::FORBIDDEN);
    assert_eq!(forbidden.header("www-authenticate"), None, "only a 401 challenges");

    let unauthenticated = app.send(Call::get("/unauthenticated")).await;
    assert_eq!(unauthenticated.status, StatusCode::UNAUTHORIZED);
    assert_eq!(unauthenticated.header("www-authenticate"), Some("Basic realm=\"vault\""), "the error's own challenge");
    app.stop().await;
}
