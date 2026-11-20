// One body consumer per handler (transports DESIGN §2.2): `Valid<Json>` beside `Form`, `Valid` forwarding what it wraps.
// Refused at: the second parameter's type, naming both parameters.
use ulo::{controller, handlers};
use ulo_http::{Form, Json};
use ulo_transport::{Valid, Validate};

#[derive(serde::Deserialize, Validate)]
struct Note {
    #[validate(length(min = 1))]
    text: String,
}

#[controller]
struct Notes;

#[handlers]
impl Notes {
    #[ulo_http::post("/notes")]
    fn create(&self, note: Valid<Json<Note>>, login: Form<String>) -> &'static str {
        let _ = (note, login);
        "created"
    }
}

fn main() {}
