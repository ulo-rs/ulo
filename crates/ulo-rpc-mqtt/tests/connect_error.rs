//! Q34-52: a client's connect that fails says so once. `RpcClient` names the link and the failed
//! connect around the link's error, so the link hands over rumqttc's own. Planted break: the link
//! wrapping rumqttc's error in "the MQTT link could not connect", which reads twice.

use std::sync::Arc;

use ulo_rpc::{ErrorKind, RpcClient};
use ulo_rpc_conformance::relay::refusing_addr;
use ulo_rpc_mqtt::Mqtt;
use ulo_runtime_tokio::Tokio;

#[tokio::test(flavor = "current_thread")]
async fn a_failed_connect_says_it_could_not_connect_once() {
    let client = RpcClient::new(Mqtt::url(format!("mqtt://{}", refusing_addr())), Arc::new(Tokio::current())).expect("the link is usable");
    let error = client.request::<_, u32>("mqtt.refused", &()).await.expect_err("a call to a closed port answered");
    assert_eq!(error.kind(), ErrorKind::Unavailable, "{}", error.message());
    let message = error.message().to_lowercase();
    assert!(message.starts_with("the mqtt link could not connect: "), "{}", error.message());
    assert_eq!(message.matches("could not connect").count(), 1, "the failure is said twice: {}", error.message());
}
