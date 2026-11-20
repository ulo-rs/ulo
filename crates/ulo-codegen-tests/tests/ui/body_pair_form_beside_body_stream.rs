// One body consumer per handler (transports DESIGN §2.2): `Form` beside `BodyStream`.
// Refused at: the second parameter's type, naming both parameters.
use ulo::{controller, handlers};
use ulo_http::{BodyStream, Form};

#[controller]
struct Notes;

#[handlers]
impl Notes {
    #[ulo_http::post("/notes")]
    fn create(&self, login: Form<String>, chunks: BodyStream) -> &'static str {
        let _ = (login, chunks);
        "created"
    }
}

fn main() {}
