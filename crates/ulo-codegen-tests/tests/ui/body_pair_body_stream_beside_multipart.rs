// One body consumer per handler (transports DESIGN §2.2): `BodyStream` beside `Multipart`.
// Refused at: the second parameter's type, naming both parameters.
use ulo::{controller, handlers};
use ulo_http::{BodyStream, Multipart};

#[controller]
struct Notes;

#[handlers]
impl Notes {
    #[ulo_http::post("/notes")]
    fn create(&self, chunks: BodyStream, parts: Multipart) -> &'static str {
        let _ = (chunks, parts);
        "created"
    }
}

fn main() {}
