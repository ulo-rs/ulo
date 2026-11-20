// One body consumer per handler (transports DESIGN §2.2): `Json` beside `Form`.
// Refused at: the second parameter's type, naming both parameters.
use ulo::{controller, handlers};
use ulo_http::{Form, Json};

#[controller]
struct Notes;

#[handlers]
impl Notes {
    #[ulo_http::post("/notes")]
    fn create(&self, note: Json<String>, login: Form<String>) -> &'static str {
        let _ = (note, login);
        "created"
    }
}

fn main() {}
