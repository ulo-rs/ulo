//! A `CallError`'s message and details reach an RPC caller through the app's redaction: a
//! registered secret's value is sent as `[redacted]` in the error envelope's message and details.
//! A secret under four characters is not matched, and a message holding no secret is sent byte
//! for byte.

use std::sync::Arc;
use std::time::Duration;

use ulo::{App, Module, ModuleDef, ModuleIdentity, Secret, Signal, controller, handlers};
use ulo_rpc::{CallError, Detail, Details, ErrorKind, Payload, RpcClient};
use ulo_rpc_tcp::Tcp;
use ulo_runtime_tokio::Tokio;

const PATIENCE: Duration = Duration::from_secs(10);
const SECRET: &str = "s3cr3t-token-xyz";
const SHORT: &str = "abc";

#[controller]
struct Keys;

#[handlers]
impl Keys {
    /// Fails with `text` as the message, the secret also in a `Json` detail when `text` is `secret`.
    #[ulo_rpc::message("keys.check")]
    fn check(&self, Payload(text): Payload<String>) -> Result<(), CallError> {
        Err(match text.as_str() {
            "secret" => CallError::new(ErrorKind::NotFound, format!("key {SECRET} refused"))
                .with_details(Details::new().with(Detail::Json(serde_json::json!({ "tried": SECRET })))),
            "short" => CallError::new(ErrorKind::NotFound, format!("key {SHORT} refused")),
            other => CallError::new(ErrorKind::NotFound, other.to_owned()),
        })
    }
}

struct Root;

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.secret(&Secret::new(SECRET));
        m.secret(&Secret::new(SHORT));
        m.controller::<Keys>();
    }
}

/// Planted break: `body_of` writing the error's message and details unredacted.
#[tokio::test]
async fn a_registered_secret_in_a_call_error_is_sent_redacted_and_a_short_one_is_not() {
    let app = App::builder(Root)
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

    let failed = |text: &'static str| {
        let client = &client;
        async move {
            let answered = tokio::time::timeout(PATIENCE, client.request::<_, ()>("keys.check", &text)).await;
            answered.expect("in time").expect_err("a refusal")
        }
    };
    let error = failed("secret").await;
    assert_eq!((error.kind(), error.message()), (ErrorKind::NotFound, "key [redacted] refused"));
    let details: Vec<&Detail> = error.details().iter().collect();
    assert_eq!(details, [&Detail::Json(serde_json::json!({ "tried": "[redacted]" }))]);
    assert_eq!(failed("short").await.message(), format!("key {SHORT} refused"));
    assert_eq!(failed("the key is spent").await.message(), "the key is spent");

    let _ = tokio::time::timeout(PATIENCE, handle.close(Signal::new("test"))).await.expect("close finished in time");
    let _ = tokio::time::timeout(PATIENCE, serving).await.expect("serve ended in time");
}
