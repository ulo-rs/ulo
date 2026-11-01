//! What the GraphQL subscription example tests share: a socket that has completed the
//! `graphql-transport-ws` handshake, and the message that starts an operation. Built on [`crate::ws`].

use std::net::SocketAddr;

use serde_json::{Value, json};

use crate::ws::{Socket, connect, next, send};

/// The subprotocol the gateway requires the upgrade to offer.
pub const SUBPROTOCOL: &str = "graphql-transport-ws";

/// An upgrade to `path` offering [`SUBPROTOCOL`], then `connection_init` and its `connection_ack`.
pub async fn ready(addr: SocketAddr, path: &str) -> Socket {
    let mut socket = connect(addr, path, &[("sec-websocket-protocol", SUBPROTOCOL)]).await;
    send(&mut socket, json!({ "type": "connection_init" })).await;
    assert_eq!(next(&mut socket).await, json!({ "type": "connection_ack" }));
    socket
}

/// Starts operation `id` running `query`.
pub async fn subscribe(socket: &mut Socket, id: &str, query: &str) {
    send(socket, json!({ "type": "subscribe", "id": id, "payload": { "query": query } })).await;
}

/// The `next` message for operation `id`, as `payload.data`.
pub async fn next_data(socket: &mut Socket, id: &str) -> Value {
    let message = next(socket).await;
    assert_eq!(message["type"], "next", "{message}");
    assert_eq!(message["id"], id, "{message}");
    message["payload"]["data"].clone()
}
