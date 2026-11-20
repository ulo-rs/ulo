// One body consumer per handler (transports DESIGN §2.2): `Json` beside `Bytes`.
// Refused at: the second parameter's type, naming both parameters.
use ulo::{controller, handlers};
use ulo_http::{Bytes, Json};

#[controller]
struct Notes;

#[handlers]
impl Notes {
    #[ulo_http::post("/notes")]
    fn create(&self, note: Json<String>, raw: Bytes) -> &'static str {
        let _ = (note, raw);
        "created"
    }
}

fn main() {}
