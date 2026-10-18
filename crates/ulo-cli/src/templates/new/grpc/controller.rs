use ulo::{Dep, injectable, routes};
use ulo_grpc::Message;

use crate::greeter::Greeter;

mod pb {
    ulo_grpc::include_proto!("greeter.v1");
}

#[injectable]
pub struct GreeterController {
    greeter: Dep<Greeter>,
}

#[routes]
impl GreeterController {
    #[ulo_grpc::method(pb::greeter::Greet)]
    fn greet(&self, request: Message<pb::GreetRequest>) -> pb::GreetReply {
        pb::GreetReply { message: self.greeter.greet(&request.0.name) }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use ulo::Dep;
    use ulo_grpc::Message;

    use super::{GreeterController, pb};
    use crate::greeter::Greeter;

    #[test]
    fn greets_the_name_in_the_request() {
        let controller = GreeterController { greeter: Dep::from_arc(Arc::new(Greeter)) };

        let reply = controller.greet(Message(pb::GreetRequest { name: "Ada".to_owned() }));

        assert_eq!(reply.message, "Hello, Ada!");
    }
}
