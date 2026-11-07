//! The close codes a connection ends with: a refused connect by its kind, 3003 for a connect
//! guard's `Ok(false)`, 3000 and 3008 for `Unauthorized` and `Timeout`, 1008 with the kind in the
//! reason for the client's other mistakes, 1013 for `TooManyRequests`, 1011 for a fault or a
//! panic, a code `OnConnect` or an error sets itself, and the drain's 1001. A refused connect is
//! answered with the Close frame alone.

use serde_json::json;
use ulo_ws::DisconnectReason;

use super::Served;
use crate::Host;
use crate::app::{
    AS_NOT_FOUND, AS_OWN_CODE, AS_TIMEOUT, AS_UNAUTHORIZED, BUSY, BY_ERROR, BY_GUARD, ERROR_CODE, FAULT, OWN, OWN_CODE, PANIC, REFUSE,
};

/// The close code and reason a connection to `/refused` refused `how` ends with.
async fn refused_with<H: Host>(app: &Served<H>, how: &str) -> Option<(u16, String)> {
    let mut conn = app.connect("/refused", &[(REFUSE, how)]).await;
    let close = conn.close_frame().await;
    conn.hang_up().await;
    close
}

pub async fn guard_refusal<H: Host>() {
    let app = Served::<H>::start().await;
    assert_eq!(refused_with(&app, BY_GUARD).await.map(|(code, _)| code), Some(3003), "a connect guard's refusal");
    app.stop().await;
}

/// A connect guard's error closes by its kind: the IANA registry's 3000 and 3008 for
/// `Unauthorized` and `Timeout`, 1008 for `NotFound` with the kind ahead of the message, and the
/// code `WithCloseCode` set in place of the kind's.
pub async fn by_kind<H: Host>() {
    let app = Served::<H>::start().await;
    assert_eq!(refused_with(&app, AS_UNAUTHORIZED).await, Some((3000, BY_ERROR.to_owned())), "`Unauthorized`");
    assert_eq!(refused_with(&app, AS_TIMEOUT).await, Some((3008, BY_ERROR.to_owned())), "`Timeout`");
    assert_eq!(refused_with(&app, AS_NOT_FOUND).await, Some((1008, format!("not_found: {BY_ERROR}"))), "`NotFound`");
    assert_eq!(refused_with(&app, AS_OWN_CODE).await, Some((ERROR_CODE, BY_ERROR.to_owned())), "`WithCloseCode`");
    app.stop().await;
}

pub async fn rate_limited<H: Host>() {
    let app = Served::<H>::start().await;
    assert_eq!(refused_with(&app, BUSY).await, Some((1013, "try again later".to_owned())), "`TooManyRequests`");
    app.stop().await;
}

pub async fn faulted<H: Host>() {
    let app = Served::<H>::start().await;
    assert_eq!(refused_with(&app, FAULT).await.map(|(code, _)| code), Some(1011), "`Internal`");
    assert_eq!(refused_with(&app, PANIC).await.map(|(code, _)| code), Some(1011), "a panic in `OnConnect`");
    app.stop().await;
}

pub async fn own_code<H: Host>() {
    let app = Served::<H>::start().await;
    assert_eq!(refused_with(&app, OWN_CODE).await, Some((OWN.0.get(), OWN.1.to_owned())), "`ConnectRefused::code`");
    app.stop().await;
}

pub async fn drain<H: Host>() {
    let app = Served::<H>::start().await;
    let mut conn = app.connect("/echo", &[]).await;
    // The 101 is written before `/echo`'s connection phase runs, and a drain that begins first
    // refuses the connection with 1001 and runs no hook, `on_disconnect` included (F385). An
    // answered message shows the phase admitted it, so the drain meets an admitted, idle connection.
    assert_eq!(conn.exchange(json!({ "event": "echo", "id": 1, "data": "admitted" })).await, json!({ "id": 1, "data": "admitted" }));
    let closing = app.close_in_background();
    assert_eq!(conn.close_frame().await, Some((1001, ulo_transport::SHUTDOWN_MESSAGE.to_owned())), "an idle connection at the drain");
    conn.hang_up().await;
    let probe = app.probe.clone();
    app.closed(closing).await;
    assert_eq!(probe.departures.snapshot(), vec![("/echo".to_owned(), DisconnectReason::Drain)], "`on_disconnect` at the drain");
}
