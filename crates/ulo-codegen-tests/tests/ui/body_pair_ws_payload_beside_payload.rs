// One body consumer per handler (transports DESIGN §2.2): two WebSocket `Payload` parameters.
// Refused at: the second parameter's type, naming both parameters.
use ulo::{controller, handlers};
use ulo_ws::Payload;

#[controller]
struct Calls;

#[handlers]
#[ulo_ws::gateway(path = "/notes")]
impl Calls {
    #[ulo_ws::message("notes.create")]
    fn call(&self, note: Payload<String>, count: Payload<u32>) {
        let _ = (note, count);
    }
}

fn main() {}
