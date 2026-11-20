// A handler parameter written as a reference to the call's context, which a handler takes by
// value (transports DESIGN §2.2).
// Refused at: the parameter's type.
use ulo::{controller, handlers};
use ulo_http::HttpCx;

#[controller]
struct Notes;

#[handlers]
impl Notes {
    #[ulo_http::get("/notes")]
    fn read(&self, cx: &HttpCx) -> String {
        cx.uri().path().to_owned()
    }
}

fn main() {}
