// One body consumer per handler (transports DESIGN §2.2): `Json` beside `Multipart`.
// Refused at: the second parameter's type, naming both parameters.
use ulo::{controller, handlers};
use ulo_http::{Json, Multipart};

#[controller]
struct Notes;

#[handlers]
impl Notes {
    #[ulo_http::post("/notes")]
    fn create(&self, note: Json<String>, parts: Multipart) -> &'static str {
        let _ = (note, parts);
        "created"
    }
}

fn main() {}
