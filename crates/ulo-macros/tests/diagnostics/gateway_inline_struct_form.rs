// The same removal on the WebSocket side: the gateway attribute takes a path,
// not a struct.
use ulo::websocket_gateway;

#[websocket_gateway("/ws", pub struct Inline {})]
pub struct Gateway {}

fn main() {}
