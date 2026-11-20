// A handler parameter whose type no transport builds from a call and the container does not
// hold: the falsification of "a parameter is read by its type" (transports DESIGN §2.2).
// Refused at: the parameter's type, with the `Param` trait's note.
use ulo::{controller, handlers};

#[controller]
struct Notes;

#[handlers]
impl Notes {
    #[ulo_http::get("/notes/{n}")]
    fn read(&self, n: u32) -> String {
        n.to_string()
    }
}

fn main() {}
