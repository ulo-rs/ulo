// One body consumer per handler (transports DESIGN §2.2): `Form` beside `Multipart`.
// Refused at: the second parameter's type, naming both parameters.
use ulo::{controller, handlers};
use ulo_http::{Form, Multipart};

#[controller]
struct Notes;

#[handlers]
impl Notes {
    #[ulo_http::post("/notes")]
    fn create(&self, login: Form<String>, parts: Multipart) -> &'static str {
        let _ = (login, parts);
        "created"
    }
}

fn main() {}
