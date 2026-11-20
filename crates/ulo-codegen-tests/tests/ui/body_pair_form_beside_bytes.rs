// One body consumer per handler (transports DESIGN §2.2): `Form` beside `Bytes`.
// Refused at: the second parameter's type, naming both parameters.
use ulo::{controller, handlers};
use ulo_http::{Bytes, Form};

#[controller]
struct Notes;

#[handlers]
impl Notes {
    #[ulo_http::post("/notes")]
    fn create(&self, login: Form<String>, raw: Bytes) -> &'static str {
        let _ = (login, raw);
        "created"
    }
}

fn main() {}
