// One body consumer per handler (transports DESIGN §2.2): `Bytes` beside `Multipart`.
// Refused at: the second parameter's type, naming both parameters.
use ulo::{controller, handlers};
use ulo_http::{Bytes, Multipart};

#[controller]
struct Notes;

#[handlers]
impl Notes {
    #[ulo_http::post("/notes")]
    fn create(&self, raw: Bytes, parts: Multipart) -> &'static str {
        let _ = (raw, parts);
        "created"
    }
}

fn main() {}
