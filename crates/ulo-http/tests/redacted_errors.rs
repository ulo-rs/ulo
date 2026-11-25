//! A `CallError`'s message and details reach the client through the app's redaction: a registered
//! secret's value is sent as `[redacted]` in the problem document's `detail` and `details` and in
//! an SSE stream's `error` event, whether a handler or an execution-scoped constructor wrote it,
//! while the error handlers read the text as written. A secret under four characters is not
//! matched, and a message holding no secret is sent byte for byte.

mod o1_support;

use std::collections::BTreeMap;
use std::sync::{Mutex, PoisonError};

use futures_util::stream;
use o1_support::{Call, start};
use ulo::{BoxError, Dep, ErrorHandler, ModuleDef, Secret, controller, handlers};
use ulo_http::{Event, Http, HttpCx, Response, Sse};
use ulo_transport::{CallError, Detail, Details, ErrorKind};

const SECRET: &str = "s3cr3t-token-xyz";
const SHORT: &str = "abc";

/// The message every error handler read, in order.
static READ: Mutex<Vec<String>> = Mutex::new(Vec::new());

struct Witness;

impl ErrorHandler<Http> for Witness {
    async fn handle(&self, err: BoxError, _cx: &HttpCx) -> Result<Response, BoxError> {
        if let Some(call) = err.downcast_ref::<CallError>() {
            READ.lock().unwrap_or_else(PoisonError::into_inner).push(call.message().to_owned());
        }
        Err(err)
    }
}

fn leaking() -> CallError {
    let metadata = BTreeMap::from([("key".to_owned(), SECRET.to_owned())]);
    CallError::new(ErrorKind::NotFound, format!("key {SECRET} refused")).with_details(
        Details::new()
            .with(Detail::error_info("refused", format!("{SECRET}.example"), metadata))
            .with(Detail::Json(serde_json::json!({ "tried": [SECRET], SECRET: 1 }))),
    )
}

/// Built per call by a constructor that fails with the secret in its message.
struct Tenant;

#[controller]
struct Leaks;

#[handlers]
#[error_handlers(value = Witness)]
impl Leaks {
    #[ulo_http::get("/handler")]
    fn handler(&self) -> Result<&'static str, CallError> {
        Err(leaking())
    }

    #[ulo_http::get("/constructor")]
    fn constructor(&self, _tenant: Dep<Tenant>) -> &'static str {
        "unreached"
    }

    #[ulo_http::get("/short")]
    fn short(&self) -> Result<&'static str, CallError> {
        Err(CallError::new(ErrorKind::Conflict, format!("key {SHORT} refused")))
    }

    #[ulo_http::get("/plain")]
    fn plain(&self) -> Result<&'static str, CallError> {
        Err(CallError::new(ErrorKind::Conflict, "the order \u{e9}tat://x changed"))
    }

    #[ulo_http::get("/late")]
    fn late(&self) -> Sse<stream::Iter<std::vec::IntoIter<Result<Event, CallError>>>> {
        Sse::new(stream::iter(vec![Ok(Event::default().data("one")), Err(leaking())]))
    }
}

fn leaks(m: &mut ModuleDef<'_>) {
    m.secret(&Secret::new(SECRET));
    m.secret(&Secret::new(SHORT));
    m.controller::<Leaks>();
    m.try_execution(|| async { Err::<Tenant, CallError>(CallError::new(ErrorKind::NotFound, format!("tenant key {SECRET} refused"))) });
}

/// Planted break: `render_as_is` writing the error as recognised, skipping `__into_sent`, which
/// sends the secret in `detail` and in `details`.
#[tokio::test]
async fn a_registered_secret_in_a_call_error_is_sent_redacted() {
    let app = start(leaks).await;

    let reply = app.send(Call::get("/handler")).await;
    assert!(!reply.text().contains(SECRET), "the secret reached the client: {}", reply.text());
    let problem = reply.json();
    assert_eq!(problem["detail"], "key [redacted] refused");
    assert_eq!(
        problem["details"],
        serde_json::json!([
            { "type": "error_info", "reason": "refused", "domain": "[redacted].example", "metadata": { "key": "[redacted]" } },
            { "type": "json", "value": { "tried": ["[redacted]"], "[redacted]": 1 } },
        ])
    );

    let reply = app.send(Call::get("/constructor")).await;
    assert_eq!(reply.json()["detail"], "tenant key [redacted] refused", "a constructor's error");

    let read = READ.lock().unwrap_or_else(PoisonError::into_inner).clone();
    assert!(read.contains(&format!("key {SECRET} refused")), "the error handlers read the message as written: {read:?}");
    app.stop().await;
}

/// A secret under four characters is not matched, and a message with no secret and no URL
/// userinfo is the bytes written. Planted break: no floor on the secret's length.
#[tokio::test]
async fn a_short_secret_and_a_message_without_one_are_sent_as_written() {
    let app = start(leaks).await;
    assert_eq!(app.send(Call::get("/short")).await.json()["detail"], format!("key {SHORT} refused"));
    assert_eq!(app.send(Call::get("/plain")).await.json()["detail"], "the order \u{e9}tat://x changed");
    app.stop().await;
}

/// Planted break: the SSE stream writing its late error unredacted.
#[tokio::test]
async fn a_registered_secret_in_an_sse_stream_s_error_event_is_sent_redacted() {
    let app = start(leaks).await;
    let text = app.send(Call::get("/late")).await.text();
    assert!(!text.contains(SECRET), "the secret reached the client: {text}");
    assert!(text.contains("key [redacted] refused"), "{text}");
    app.stop().await;
}
