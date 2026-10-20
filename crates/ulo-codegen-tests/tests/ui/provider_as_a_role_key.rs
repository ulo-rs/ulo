// §12 row: "`X as <role key>` in a `providers` list" (compile). Capabilities 28, 46.
// Refused at: the role key, "a global enhancer is written `into AnyGuard<Http>: [AuthGuard]`".
use ulo::{BoxError, Guard, injectable, module};
use ulo_http::{Http, HttpCx};

#[injectable]
struct AuthGuard;

impl Guard<Http> for AuthGuard {
    async fn can_activate(&self, _cx: &HttpCx) -> Result<bool, BoxError> {
        Ok(true)
    }
}

#[module(providers = [AuthGuard as ulo::AnyGuard<Http>])]
struct Auth;

fn main() {}
