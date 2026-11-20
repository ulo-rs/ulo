// One body consumer per handler (transports DESIGN §2.2): `Json` beside `BodyStream`.
// Refused at: the second parameter's type, naming both parameters.
use ulo::{controller, handlers};
use ulo_http::{BodyStream, Json};

#[controller]
struct Notes;

#[handlers]
impl Notes {
    #[ulo_http::post("/notes")]
    fn create(&self, note: Json<String>, chunks: BodyStream) -> &'static str {
        let _ = (note, chunks);
        "created"
    }
}

fn main() {}
