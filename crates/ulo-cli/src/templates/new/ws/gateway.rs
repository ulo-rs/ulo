use serde::{Deserialize, Serialize};
use ulo::{Dep, injectable, routes};
use ulo_ws::Payload;

use crate::greeter::Greeter;

#[derive(Debug, Deserialize)]
pub struct Greet {
    pub name: String,
}

#[derive(Debug, Serialize)]
pub struct Greeting {
    pub message: String,
}

#[injectable]
pub struct GreetingGateway {
    greeter: Dep<Greeter>,
}

#[routes]
#[ulo_ws::gateway(path = "/ws")]
impl GreetingGateway {
    #[ulo_ws::message("greet")]
    fn greet(&self, request: Payload<Greet>) -> Greeting {
        Greeting { message: self.greeter.greet(&request.0.name) }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use ulo::Dep;
    use ulo_ws::Payload;

    use super::{Greet, GreetingGateway};
    use crate::greeter::Greeter;

    #[test]
    fn greets_the_name_in_the_message() {
        let gateway = GreetingGateway { greeter: Dep::from_arc(Arc::new(Greeter)) };

        let reply = gateway.greet(Payload(Greet { name: "Ada".to_owned() }));

        assert_eq!(reply.message, "Hello, Ada!");
    }
}
