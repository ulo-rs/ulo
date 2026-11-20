// One body consumer per handler (transports DESIGN §2.2): two `Json` parameters.
// Refused at: the second parameter's type, naming both parameters.
use ulo::{controller, handlers};
use ulo_http::{Json};

#[controller]
struct Notes;

#[handlers]
impl Notes {
    #[ulo_http::post("/notes")]
    fn create(&self, note: Json<String>, count: Json<u32>) -> &'static str {
        let _ = (note, count);
        "created"
    }
}

fn main() {}
