use serde::Serialize;
use ulo::{Dep, injectable, routes};
use ulo_http::{Json, Path};

use crate::greeter::Greeter;

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
    #[ulo_http::get("/greetings/{name}")]
    fn greet(&self, name: Path<String>) -> Json<Greeting> {
        Json(Greeting { message: self.greeter.greet(&name.0) })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use ulo::Dep;
    use ulo_http::Path;

    use super::GreetingController;
    use crate::greeter::Greeter;

    #[test]
    fn greets_the_name_in_the_path() {
        let controller = GreetingController { greeter: Dep::from_arc(Arc::new(Greeter)) };

        let reply = controller.greet(Path("Ada".to_owned()));

        assert_eq!(reply.0.message, "Hello, Ada!");
    }
}
