use serde::Deserialize;
use ulo::{Dep, injectable, routes};
use ulo_http::{Json, Path};
use ulo_ws::{BroadcastError, Rooms};

#[derive(Deserialize)]
pub struct Notice {
    pub text: String,
}

/// An HTTP route pushing to WebSocket clients: `Rooms` is an ordinary injectable, so any
/// controller on any transport can broadcast through the same adapter the gateway uses.
#[injectable]
pub struct Notifier {
    rooms: Dep<Rooms>,
}

#[routes]
impl Notifier {
    /// Delivers `text` to the members of `room` as a `notice` event.
    #[ulo_http::post("/notify/{room}")]
    async fn notify(&self, Path(room): Path<String>, Json(notice): Json<Notice>) -> Result<&'static str, BroadcastError> {
        self.rooms.to_room(room).emit("notice", &notice.text).await?;
        Ok("sent")
    }
}
