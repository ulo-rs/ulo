// One body consumer per handler (transports DESIGN §2.2): `Bytes` beside `BodyStream`.
// Refused at: the second parameter's type, naming both parameters.
use ulo::{controller, handlers};
use ulo_http::{BodyStream, Bytes};

#[controller]
struct Notes;

#[handlers]
impl Notes {
    #[ulo_http::post("/notes")]
    fn create(&self, raw: Bytes, chunks: BodyStream) -> &'static str {
        let _ = (raw, chunks);
        "created"
    }
}

fn main() {}
