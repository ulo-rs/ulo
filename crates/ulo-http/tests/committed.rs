//! An answer the backend has taken whole is committed: its execution, still held by the response
//! body when the drain ends, is cancelled and not counted in `Shutdown::abandoned`, and so is an
//! answer with no body, whose head is all there is to write. An answer whose body has not been
//! written to its end is abandoned, and so is one whose body ended in an error frame, a reply cut
//! off rather than written. The drain window is zero, so the drain ends as it begins, with
//! every body held by the test.

mod o1_support;

use std::future::poll_fn;
use std::pin::Pin;
use std::time::Duration;

use bytes::Bytes;
use http_body::Body as _;
use o1_support::{Call, start_draining};
use ulo::{ModuleDef, controller, handlers};
use ulo_http::{HttpBody, NoContent, Response, StatusCode};

#[controller]
struct Answers;

#[handlers]
impl Answers {
    #[ulo_http::get("/answer")]
    fn answer(&self) -> &'static str {
        "answered"
    }

    #[ulo_http::get("/nothing")]
    fn nothing(&self) -> NoContent {
        NoContent
    }

    #[ulo_http::get("/cut")]
    fn cut(&self) -> Response {
        let chunks = [Ok(Bytes::from_static(b"partial")), Err(std::io::Error::other("the source failed"))];
        Response::new(HttpBody::stream(futures_util::stream::iter(chunks)))
    }
}

fn register(m: &mut ModuleDef<'_>) {
    m.controller::<Answers>();
}

#[tokio::test]
async fn an_answer_written_to_its_end_is_not_abandoned_and_one_unwritten_is() {
    let app = start_draining(register, Duration::ZERO).await;

    let written = app.call(Call::get("/answer")).await;
    assert_eq!(written.status(), StatusCode::OK);
    let mut written = written.into_body();
    let mut bytes = Vec::new();
    while let Some(frame) = poll_fn(|cx| Pin::new(&mut written).poll_frame(cx)).await {
        if let Ok(data) = frame.expect("the body yields no error").into_data() {
            bytes.extend_from_slice(&data);
        }
    }
    assert_eq!(bytes, b"answered");
    let unwritten = app.call(Call::get("/answer")).await;
    let bodiless = app.call(Call::get("/nothing")).await;
    assert_eq!(bodiless.status(), StatusCode::NO_CONTENT);
    let mut cut = app.call(Call::get("/cut")).await.into_body();
    let mut frames = Vec::new();
    while let Some(frame) = poll_fn(|cx| Pin::new(&mut cut).poll_frame(cx)).await {
        frames.push(frame.is_ok());
    }
    assert_eq!(frames, [true, false], "a data frame, then the error that cut the body off");

    let report = app.begin_close().await.expect("close did not panic").expect("shutdown reported a failure");
    assert_eq!(
        report.abandoned, 2,
        "the answers the backend took whole are not abandoned; the one it never read and the one cut off by an error are"
    );
    drop((written, unwritten, bodiless, cut));
    app.stop().await;
}
