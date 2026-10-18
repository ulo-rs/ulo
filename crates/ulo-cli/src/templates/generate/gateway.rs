use ulo::{injectable, routes};

/// A WebSocket gateway at `/@@route@@`, served on the HTTP server's port once the application
/// imports `ulo_ws::WsModule::for_root()`. Bind it with `m.controller::<@@pascal@@Gateway>()`.
#[injectable]
pub struct @@pascal@@Gateway;

#[routes]
#[ulo_ws::gateway(path = "/@@route@@")]
impl @@pascal@@Gateway {
    #[ulo_ws::message("ping")]
    fn ping(&self) -> &'static str {
        "pong"
    }
}

#[cfg(test)]
mod tests {
    use super::@@pascal@@Gateway;

    #[test]
    fn answers_ping() {
        assert_eq!(@@pascal@@Gateway.ping(), "pong");
    }
}
