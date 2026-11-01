// §12 row: "An enhancer that lacks its role trait, including a controller-level one for any
// handler's transport" (compile). Capabilities 25, 46.
// Refused at: the impl-level `#[guards(..)]` entry, once for the handler whose transport (RPC)
// the guard has no role for; the HTTP handler is satisfied.
use ulo::{BoxError, Guard, injectable, routes};
use ulo_http::{Http, HttpCx};
use ulo_rpc::Payload;

struct HttpOnly;

impl Guard<Http> for HttpOnly {
    async fn can_activate(&self, _cx: &HttpCx) -> Result<bool, BoxError> {
        Ok(true)
    }
}

#[injectable]
struct Orders;

#[routes]
#[guards(HttpOnly)]
impl Orders {
    #[ulo_http::get("/orders")]
    async fn list(&self) -> &'static str {
        "orders"
    }

    #[ulo_rpc::message("orders.get")]
    async fn get(&self, id: Payload<u64>) -> u64 {
        id.0
    }
}

fn main() {}
