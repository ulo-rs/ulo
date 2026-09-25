//! A `Binary` payload is refused by the tcp and udp clients before any I/O. Their wire carries
//! JSON, which has no encoding for it; a call that succeeded and handed the server `null` would
//! report a delivery that never happened.
//!
//! The peer in each case is a bare socket that must see nothing: no connection attempt on tcp, no
//! datagram on udp.

use std::time::Duration;

use ulo::rpc::{RpcClient, RpcClientError, RpcData};

async fn refusals(client: &RpcClient) -> [Result<(), RpcClientError>; 3] {
    [
        client
            .send("any.pattern", RpcData::binary(b"{\"a\":1}".to_vec()))
            .await
            .map(|_| ()),
        client
            .emit("any.pattern", RpcData::binary(vec![0xFF, 0xFE]))
            .await,
        client
            .stream("any.pattern", RpcData::binary(vec![1]))
            .await
            .map(|_| ()),
    ]
}

#[tokio::test]
async fn tcp_refuses_a_binary_payload_before_connecting() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let client = RpcClient::new(
        ulo_rpc_tcp::TcpClientTransport::new("127.0.0.1", port)
            .with_timeout(Duration::from_secs(1)),
    );

    for outcome in refusals(&client).await {
        assert!(
            matches!(outcome, Err(RpcClientError::Transport(_))),
            "expected the client's own refusal, got {outcome:?}"
        );
    }

    let accepted = tokio::time::timeout(Duration::from_millis(300), listener.accept()).await;
    assert!(
        accepted.is_err(),
        "the client opened a connection for a payload it refused"
    );
}

#[tokio::test]
async fn udp_refuses_a_binary_payload_before_sending() {
    let socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let port = socket.local_addr().unwrap().port();
    let client = RpcClient::new(
        ulo_rpc_udp::UdpClientTransport::new("127.0.0.1", port)
            .with_timeout(Duration::from_secs(1)),
    );

    for outcome in refusals(&client).await {
        assert!(
            matches!(outcome, Err(RpcClientError::Transport(_))),
            "expected the client's own refusal, got {outcome:?}"
        );
    }

    let mut buf = [0u8; 64];
    let received = tokio::time::timeout(Duration::from_millis(300), socket.recv(&mut buf)).await;
    assert!(
        received.is_err(),
        "the client sent a datagram for a payload it refused"
    );
}
