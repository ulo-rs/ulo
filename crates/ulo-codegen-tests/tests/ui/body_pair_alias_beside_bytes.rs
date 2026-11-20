// One body consumer per handler (transports DESIGN §2.2): a `type` alias of `Json` beside `Bytes`, the alias consuming what `Json` does.
// Refused at: the second parameter's type, naming both parameters.
use ulo::{controller, handlers};
use ulo_http::{Bytes, Json};

type Body<T> = Json<T>;

#[controller]
struct Notes;

#[handlers]
impl Notes {
    #[ulo_http::post("/notes")]
    fn create(&self, note: Body<String>, raw: Bytes) -> &'static str {
        let _ = (note, raw);
        "created"
    }
}

fn main() {}
