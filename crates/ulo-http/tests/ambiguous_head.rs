//! An HTTP/1.x request with two `Host` lines, or `Content-Length` beside `Transfer-Encoding`, is
//! answered 400 with `Connection: close` before an execution opens (transports DESIGN decision
//! 281). On the wire most hosts never hand the service the second case (hyper drops the
//! `Content-Length`, actix refuses the request itself), so the service's own check is held here.

mod o1_support;

use http::{StatusCode, Version};
use o1_support::{Call, read, start};
use ulo::{ModuleDef, controller, handlers};
use ulo_http::Routing;

#[controller]
struct Hit;

#[handlers]
impl Hit {
    #[ulo_http::get("/hit")]
    fn hit(&self) -> &'static str {
        "hit"
    }
}

/// A named request, its request builder and the detail its 400 carries.
type Case = (&'static str, fn() -> Call, &'static str);

fn register(m: &mut ModuleDef<'_>) {
    m.controller::<Hit>();
}

#[tokio::test]
async fn two_host_lines_or_both_length_headers_answer_400_and_close_on_http_1_alone() {
    let app = start(register).await;
    let cases: [Case; 2] = [
        ("two `Host` lines", || Call::get("/hit").header("host", "a.example").header("host", "a.example"), "the request carries more than one `Host` line"),
        (
            "`Content-Length` beside `Transfer-Encoding`",
            || Call::get("/hit").header("host", "a.example").header("content-length", "0").header("transfer-encoding", "chunked"),
            "the request carries `Content-Length` beside `Transfer-Encoding`",
        ),
    ];
    for (what, call, detail) in cases {
        for version in [Version::HTTP_10, Version::HTTP_11, Version::HTTP_2] {
            let mut req = call().into_request();
            req.head.version = version;
            let response = app.service().call(req).await;
            let answered = response.version();
            let reply = read(response).await;
            if version == Version::HTTP_2 {
                assert_eq!(reply.status, StatusCode::OK, "{what} over {version:?} is not checked");
                continue;
            }
            assert_eq!(reply.status, StatusCode::BAD_REQUEST, "{what} over {version:?}: {}", reply.text());
            assert_eq!(reply.header("connection"), Some("close"), "{what} over {version:?}");
            assert_eq!(answered, version, "{what}: the 400 is written in the request's version, which hyper reads to end the connection");
            assert_eq!(reply.json()["detail"], detail, "{what} over {version:?}");
            assert!(matches!(reply.routing, Some(Routing::Unrouted)), "{what} over {version:?}: {:?}", reply.routing);
        }
    }
    let one = app.send(Call::get("/hit").header("host", "a.example")).await;
    assert_eq!(one.status, StatusCode::OK, "one `Host` line");
    app.stop().await;
}
