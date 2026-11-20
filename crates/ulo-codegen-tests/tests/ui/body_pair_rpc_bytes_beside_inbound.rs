// One body consumer per handler (transports DESIGN §2.2): an RPC `Bytes` beside `Inbound`.
// Refused at: the second parameter's type, naming both parameters.
use ulo::{controller, handlers};
use ulo_rpc::{Bytes, Inbound, Payload};

#[controller]
struct Calls;

#[handlers]
impl Calls {
    #[ulo_rpc::message("notes.create")]
    fn call(&self, raw: Bytes, notes: Inbound<String>) {
        let _ = (raw, notes);
    }
}

fn main() {}
