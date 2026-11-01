//! Scenarios: pre dispatch.

use http::Method;

use crate::app::KEY_HEADER;
use crate::wire::{Exchange, is_reference, same_as_reference, start};
use crate::{Host, Mode, ORIGIN};

/// An unscoped entry answering a CORS preflight without calling `next`, `Routing::Unrouted`.
pub async fn preflight<H: Host>(mode: Mode) {
    let host = start::<H>(mode).await;
    let exchange = Exchange::new(Method::OPTIONS, "/hit")
        .header("origin", ORIGIN)
        .header("access-control-request-method", "GET");
    let reply = same_as_reference(&host, exchange).await;
    assert_eq!(reply.status, 204);
    assert_eq!(reply.header("access-control-allow-origin"), Some(ORIGIN));
    if !is_reference::<H>() {
        assert_eq!(reply.routing(), Some("unrouted"));
    }
    host.stop().await;
}

/// The scoped entry over `/guarded/*` answers 401 for a path under it that no route matches as
/// for the one a route matches, decorated by the CORS entry outside it; with its key, routing
/// answers each.
pub async fn scoped_unrouted<H: Host>(mode: Mode) {
    let host = start::<H>(mode).await;
    for (path, routing) in [("/guarded/hit", format!("matched {}", host.route("/guarded/hit"))), ("/guarded/nowhere", "not-found".to_owned())] {
        let reply = same_as_reference(&host, Exchange::get(path).header("origin", ORIGIN)).await;
        assert_eq!(reply.status, 401, "GET {path}: {}", reply.text());
        assert_eq!(reply.header("access-control-allow-origin"), Some(ORIGIN), "GET {path}");
        if !is_reference::<H>() {
            assert_eq!(reply.routing(), Some(routing.as_str()), "GET {path}");
        }
    }
    for (path, status) in [("/guarded/hit", 200), ("/guarded/nowhere", 404)] {
        let reply = same_as_reference(&host, Exchange::get(path).header(KEY_HEADER, "1")).await;
        assert_eq!(reply.status, status, "GET {path} with the key: {}", reply.text());
    }
    host.stop().await;
}
