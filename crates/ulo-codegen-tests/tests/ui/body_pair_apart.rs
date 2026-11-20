// One body consumer per handler (transports DESIGN §2.2): `Json` and `Bytes` with a parameter that
// reads no body between them, so a check of neighbouring parameters alone passes the pair.
// Refused at: the third parameter's type, naming the first and the third.
use ulo::{controller, handlers};
use ulo_http::{Bytes, Json, Path};

#[controller]
struct Notes;

#[handlers]
impl Notes {
    #[ulo_http::post("/notes/{id}")]
    fn create(&self, note: Json<String>, id: Path<u32>, raw: Bytes) -> &'static str {
        let _ = (note, id, raw);
        "created"
    }
}

fn main() {}
