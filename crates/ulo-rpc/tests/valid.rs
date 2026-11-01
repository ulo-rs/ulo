//! `Valid<Payload<T>>` on a `#[message]` handler: a payload that passes reaches the handler, one
//! that fails is answered `unprocessable` with a `FieldViolations` detail naming each violated
//! field, and the handler does not run. Capability: `Valid` on RPC (G20).

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use ulo::{App, AppHandle, Dep, Module, ModuleDef, ModuleIdentity, Signal, injectable, routes};
use ulo_rpc::{Payload, RpcClient, RpcError};
use ulo_rpc_tcp::Tcp;
use ulo_runtime_tokio::Tokio;
use ulo_transport::{Detail, ErrorKind, FieldViolation, Valid, Validate};

const PATIENCE: Duration = Duration::from_secs(10);

#[derive(Serialize, Deserialize, Validate)]
struct Signup {
    #[validate(length(min = 3))]
    name: String,
    #[validate(range(min = 18))]
    age: u32,
}

/// The names the handler was called with.
#[derive(Clone, Default)]
struct Ran(Arc<Mutex<Vec<String>>>);

impl Ran {
    fn names(&self) -> Vec<String> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

#[injectable]
struct Forms {
    ran: Dep<Ran>,
}

#[routes]
impl Forms {
    #[ulo_rpc::message("forms.signup")]
    fn signup(&self, form: Valid<Payload<Signup>>) -> String {
        let name = form.0.0.name.clone();
        self.ran.0.lock().unwrap_or_else(PoisonError::into_inner).push(name.clone());
        format!("welcome {name}")
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

struct Running {
    handle: AppHandle,
    serving: tokio::task::JoinHandle<()>,
    client: RpcClient,
}

async fn start(ran: Ran) -> Running {
    let app = App::builder(Root(ran))
        .runtime(Tokio::current())
        .wire()
        .expect("the app wires")
        .connect()
        .await
        .expect("the app connects")
        .bind(ulo_rpc::Server::new(Tcp::new("127.0.0.1:0")))
        .listen()
        .await
        .expect("the server binds");
    let addr = app.addresses().first().map(|bound| bound.addr).expect("the server's address");
    let handle = app.handle();
    let serving = tokio::spawn(async move {
        let _ = app.serve(std::future::pending::<Signal>()).await;
    });
    let client = RpcClient::new(Tcp::new(addr.to_string()), Arc::new(Tokio::current())).expect("a client on the bound address");
    Running { handle, serving, client }
}

impl Running {
    async fn signup(&self, name: &str, age: u32) -> Result<String, RpcError> {
        let form = Signup { name: name.to_owned(), age };
        tokio::time::timeout(PATIENCE, self.client.request::<Signup, String>("forms.signup", &form)).await.expect("the call answered in time")
    }

    async fn stop(self) {
        let _ = tokio::time::timeout(PATIENCE, self.handle.close(Signal::new("test"))).await.expect("close finished in time");
        let _ = tokio::time::timeout(PATIENCE, self.serving).await.expect("serve ended in time");
    }
}

/// The error's one `FieldViolations` detail, sorted by field.
fn violated(error: &RpcError) -> Vec<FieldViolation> {
    let details: Vec<&Detail> = error.details().iter().collect();
    let [Detail::FieldViolations(violations)] = details.as_slice() else {
        panic!("expected one FieldViolations detail, got {details:?}");
    };
    sorted(violations.clone())
}

/// What `Signup`'s own validation reports for the form, sorted by field.
fn violations_of(name: &str, age: u32) -> Vec<FieldViolation> {
    sorted(Signup { name: name.to_owned(), age }.validate().expect_err("the form is invalid"))
}

fn sorted(mut violations: Vec<FieldViolation>) -> Vec<FieldViolation> {
    violations.sort_by(|a, b| a.field.cmp(&b.field));
    violations
}

#[tokio::test]
async fn a_valid_payload_reaches_the_handler() {
    let ran = Ran::default();
    let app = start(ran.clone()).await;
    assert_eq!(app.signup("ada", 36).await.expect("a valid payload is answered"), "welcome ada");
    assert_eq!(ran.names(), vec!["ada".to_owned()]);
    app.stop().await;
}

#[tokio::test]
async fn an_invalid_payload_is_unprocessable_with_each_violated_field_and_the_handler_does_not_run() {
    let ran = Ran::default();
    let app = start(ran.clone()).await;
    let both = app.signup("al", 12).await.expect_err("an invalid payload was answered");
    assert_eq!(both.kind(), ErrorKind::Unprocessable, "{both}");
    let fields: Vec<String> = violated(&both).into_iter().map(|violation| violation.field).collect();
    assert_eq!(fields, ["age", "name"]);
    assert_eq!(violated(&both), violations_of("al", 12), "each violation with its description");
    let one = app.signup("ada", 12).await.expect_err("an invalid payload was answered");
    assert_eq!(violated(&one), violations_of("ada", 12), "only the failing field is named");
    assert_eq!(violated(&one)[0].field, "age");
    assert!(ran.names().is_empty(), "the handler ran for an invalid payload");
    app.stop().await;
}
