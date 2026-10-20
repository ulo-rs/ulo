// §12 row: "An enhancer that lacks its role trait" (compile), a method-level guard that is no
// guard for the handler's transport. Capabilities 25, 46.
// Refused at: the `#[guards(..)]` entry, by `Guard<Http>`, with a note naming the handler.
use ulo::{injectable, routes};

struct NotAGuard;

#[injectable]
struct Orders;

#[routes]
impl Orders {
    #[ulo_http::get("/orders")]
    #[guards(NotAGuard)]
    async fn list(&self) -> &'static str {
        "orders"
    }
}

fn main() {}
