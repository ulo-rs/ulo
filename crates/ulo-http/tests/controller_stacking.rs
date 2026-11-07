//! `#[controller]` and `#[handlers]` on one impl, in either order: the constructor, `new` or a
//! `#[construct]` fn, builds the controller from its parameters, and the handlers beside it are
//! mounted. Each controller is listed under a `#[module]`'s `controllers`, which accepts only a
//! `#[controller]` type.

mod o1_support;

use http::StatusCode;
use o1_support::{Call, start};
use ulo::{Dep, controller, handlers, module};

/// What every controller here reads through its constructor, so a reply carrying it shows the
/// constructor ran with its parameter injected.
struct Greeting(&'static str);

struct ControllerFirst {
    greeting: Dep<Greeting>,
}

#[controller]
#[handlers]
impl ControllerFirst {
    fn new(greeting: Dep<Greeting>) -> Self {
        ControllerFirst { greeting }
    }

    #[ulo_http::get("/controller-first")]
    fn get(&self) -> String {
        format!("{} from controller-first", self.greeting.0)
    }
}

struct HandlersFirst {
    greeting: Dep<Greeting>,
}

#[handlers]
#[controller]
impl HandlersFirst {
    fn new(greeting: Dep<Greeting>) -> Self {
        HandlersFirst { greeting }
    }

    #[ulo_http::get("/handlers-first")]
    fn get(&self) -> String {
        format!("{} from handlers-first", self.greeting.0)
    }
}

struct ConstructedControllerFirst {
    greeting: Dep<Greeting>,
}

#[controller]
#[handlers]
impl ConstructedControllerFirst {
    #[construct]
    fn build(greeting: Dep<Greeting>) -> Self {
        ConstructedControllerFirst { greeting }
    }

    #[ulo_http::get("/constructed-controller-first")]
    fn get(&self) -> String {
        format!("{} from constructed-controller-first", self.greeting.0)
    }
}

/// `#[handlers]` expands first and sees `#[construct]` still on `build`: the marker does not make
/// it a handler, and `#[controller]` then takes it.
struct ConstructedHandlersFirst {
    greeting: Dep<Greeting>,
}

#[handlers]
#[controller]
impl ConstructedHandlersFirst {
    #[construct]
    fn build(greeting: Dep<Greeting>) -> Self {
        ConstructedHandlersFirst { greeting }
    }

    #[ulo_http::get("/constructed-handlers-first")]
    fn get(&self) -> String {
        format!("{} from constructed-handlers-first", self.greeting.0)
    }
}

#[module(
    providers = [Greeting("hello")],
    controllers = [ControllerFirst, HandlersFirst, ConstructedControllerFirst, ConstructedHandlersFirst],
)]
struct Stacked;

#[tokio::test]
async fn controller_and_handlers_stack_on_one_impl_in_either_order() {
    let app = start(|m| {
        m.import(Stacked);
    })
    .await;
    for name in ["controller-first", "handlers-first", "constructed-controller-first", "constructed-handlers-first"] {
        let reply = app.send(Call::get(&format!("/{name}"))).await;
        assert_eq!(
            (reply.status, reply.text()),
            (StatusCode::OK, format!("hello from {name}")),
            "the handler of the controller stacked as {name}"
        );
    }
    app.stop().await;
}
