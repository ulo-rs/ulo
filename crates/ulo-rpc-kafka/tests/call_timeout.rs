//! A call on the Kafka client returns within the timeout its caller configured, publish
//! included.
//!
//! The producer's own delivery wait is not the caller's bound: with the broker gone, a publish
//! waits on rdkafka's delivery timeout, far past any `with_timeout` a caller sets. Each case
//! connects to a live broker, stops it, and times one call.
//!
//! Needs a live Kafka from testcontainers, so it is gated behind the `integration` feature.
#![cfg(feature = "integration")]

use std::time::{Duration, Instant};

use testcontainers::runners::AsyncRunner;
use testcontainers_modules::kafka::apache::{KAFKA_PORT, Kafka};
use ulo::rpc::{RpcClient, RpcClientError, RpcData};
use ulo_rpc_kafka::KafkaClientTransport;

const TIMEOUT: Duration = Duration::from_millis(500);
/// Room above `TIMEOUT` for scheduling; well under anything the broker's absence would cause.
const BOUND: Duration = Duration::from_millis(1500);

/// A connected client whose broker has just stopped.
async fn client_without_its_broker() -> RpcClient {
    let container = Kafka::default()
        .start()
        .await
        .expect("a kafka container starts");
    let port = container
        .get_host_port_ipv4(KAFKA_PORT)
        .await
        .expect("kafka publishes its port");
    let client = RpcClient::new(
        KafkaClientTransport::new(format!("127.0.0.1:{port}")).with_timeout(TIMEOUT),
    );
    client.connect().await.expect("the client connects");
    container.stop().await.expect("the container stops");
    client
}

/// Runs `call`, giving up well past `BOUND` so a call the timeout does not bound fails the
/// assertion rather than hanging the suite.
async fn timed<F: std::future::Future>(call: F) -> (Duration, Option<F::Output>) {
    let started = Instant::now();
    let answer = tokio::time::timeout(Duration::from_secs(10), call)
        .await
        .ok();
    (started.elapsed(), answer)
}

#[tokio::test]
async fn a_send_answers_timeout_within_its_timeout() {
    let client = client_without_its_broker().await;
    let (took, answer) =
        timed(client.send("orders.create", RpcData::json(serde_json::json!({})))).await;
    assert!(
        took < BOUND,
        "send took {took:?} with a {TIMEOUT:?} timeout"
    );
    assert!(
        matches!(answer, Some(Err(RpcClientError::Timeout))),
        "send answered {answer:?}"
    );
}

#[tokio::test]
async fn an_emit_answers_timeout_within_its_timeout() {
    let client = client_without_its_broker().await;
    let (took, answer) =
        timed(client.emit("orders.created", RpcData::json(serde_json::json!({})))).await;
    assert!(
        took < BOUND,
        "emit took {took:?} with a {TIMEOUT:?} timeout"
    );
    assert!(
        matches!(answer, Some(Err(RpcClientError::Timeout))),
        "emit answered {answer:?}"
    );
}

#[tokio::test]
async fn a_stream_fails_to_open_within_its_timeout() {
    let client = client_without_its_broker().await;
    let (took, answer) =
        timed(client.stream("orders.watch", RpcData::json(serde_json::json!({})))).await;
    assert!(
        took < BOUND,
        "stream took {took:?} with a {TIMEOUT:?} timeout"
    );
    assert!(
        matches!(answer, Some(Err(RpcClientError::Timeout))),
        "stream answered {}",
        match &answer {
            None => "nothing".to_string(),
            Some(Ok(_)) => "an open stream".to_string(),
            Some(Err(e)) => format!("{e:?}"),
        }
    );
}
