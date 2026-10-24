//! `Valid<Payload<T>>` on a `#[message]` handler: a payload that passes reaches the handler, one
//! that fails is answered as an `unprocessable` error envelope naming each violated field, and
//! the handler does not run. Capability: `Valid` on WebSocket (G12).

mod support;

use serde::Deserialize;
use serde_json::json;
use ulo::{Dep, Module, ModuleDef, ModuleIdentity, injectable, routes};
use ulo_transport::{Valid, Validate};
use ulo_ws::Payload;

use support::{Record, Running, hang_up, next_json, send_json};

#[derive(Deserialize, Validate)]
struct Signup {
    #[validate(length(min = 3))]
    name: String,
    #[validate(range(min = 18))]
    age: u32,
}

#[derive(Clone)]
struct Ran(Record<String>);

#[injectable]
struct Forms {
    ran: Dep<Ran>,
}

#[routes]
#[ulo_ws::gateway(path = "/forms", port = own)]
impl Forms {
    #[ulo_ws::message("signup")]
    fn signup(&self, form: Valid<Payload<Signup>>) -> String {
        self.ran.0.push(form.0.0.name.clone());
        format!("welcome {}", form.0.0.name)
    }
}

struct Root(Ran);

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.value(self.0.clone());
        m.controller::<Forms>();
    }
}

#[tokio::test]
async fn a_valid_payload_reaches_the_handler() {
    let ran = Ran(Record::new());
    let app = Running::start(Root(ran.clone()), ulo_ws_hyper::Server::new("127.0.0.1:0")).await;
    let mut socket = app.connect("/forms", &[]).await;
    send_json(&mut socket, &json!({ "event": "signup", "id": 1, "data": { "name": "ada", "age": 36 } })).await;
    assert_eq!(next_json(&mut socket).await, json!({ "id": 1, "data": "welcome ada" }));
    assert_eq!(ran.0.snapshot(), vec!["ada".to_owned()]);
    hang_up(socket).await;
    app.stop().await;
}

#[tokio::test]
async fn an_invalid_payload_is_unprocessable_with_each_violated_field_and_the_handler_does_not_run() {
    let ran = Ran(Record::new());
    let app = Running::start(Root(ran.clone()), ulo_ws_hyper::Server::new("127.0.0.1:0")).await;
    let mut socket = app.connect("/forms", &[]).await;
    send_json(&mut socket, &json!({ "event": "signup", "id": 7, "data": { "name": "al", "age": 12 } })).await;
    let reply = next_json(&mut socket).await;
    assert_eq!(reply["id"], json!(7), "{reply}");
    assert_eq!(reply["error"]["kind"], json!("unprocessable"), "{reply}");
    let details = reply["error"]["details"].as_array().unwrap_or_else(|| panic!("no details in {reply}"));
    assert_eq!(details.len(), 1, "{reply}");
    assert_eq!(details[0]["type"], json!("field_violations"), "{reply}");
    let mut fields: Vec<&str> = details[0]["violations"]
        .as_array()
        .unwrap_or_else(|| panic!("no violations in {reply}"))
        .iter()
        .filter_map(|violation| violation["field"].as_str())
        .collect();
    fields.sort_unstable();
    assert_eq!(fields, vec!["age", "name"], "{reply}");
    assert!(ran.0.snapshot().is_empty(), "the handler ran for an invalid payload");

    send_json(&mut socket, &json!({ "event": "signup", "id": 8, "data": { "name": "ada", "age": 12 } })).await;
    let reply = next_json(&mut socket).await;
    let violations = reply["error"]["details"][0]["violations"].as_array().unwrap_or_else(|| panic!("no violations in {reply}"));
    assert_eq!(violations.len(), 1, "only the failing field is named: {reply}");
    assert_eq!(violations[0]["field"], json!("age"));
    hang_up(socket).await;
    app.stop().await;
}
