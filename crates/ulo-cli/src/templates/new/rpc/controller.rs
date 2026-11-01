use serde::{Deserialize, Serialize};
use ulo::{Dep, injectable, routes};
use ulo_rpc::Payload;

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
pub struct GreetingController {
    greeter: Dep<Greeter>,
}

#[routes]
impl GreetingController {
    #[ulo_rpc::message("greeter.greet")]
    async fn greet(&self, request: Payload<Greet>) -> Greeting {
        Greeting { message: self.greeter.greet(&request.0.name) }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use ulo::Dep;
    use ulo_rpc::Payload;

    use super::{Greet, GreetingController};
    use crate::greeter::Greeter;

    #[tokio::test]
    async fn greets_the_name_in_the_payload() {
        let controller = GreetingController { greeter: Dep::from_arc(Arc::new(Greeter)) };

        let reply = controller.greet(Payload(Greet { name: "Ada".to_owned() })).await;

        assert_eq!(reply.message, "Hello, Ada!");
    }
}
