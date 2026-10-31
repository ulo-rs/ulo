use ulo::{BoxError, Guard};
use ulo_ws::{ConnectCx, WsConnect};

/// The header a visitor may carry a badge in on the upgrade request.
pub const BADGE: &str = "x-badge";

/// What the connect guard learned from the upgrade. It rides the connect phase's extensions to
/// `OnConnect` only: each message is an execution of its own, so a message reads it from the
/// session, where `on_connect` put it.
pub struct Badge(pub String);

/// Admits every upgrade and leaves the visitor's badge for `on_connect`.
pub struct Admit;

impl Guard<WsConnect> for Admit {
    async fn can_activate(&self, cx: &ConnectCx) -> Result<bool, BoxError> {
        let badge = cx.head().headers().get(BADGE).and_then(|badge| badge.to_str().ok()).unwrap_or("guest");
        cx.extensions().insert(Badge(badge.to_owned()));
        Ok(true)
    }
}
