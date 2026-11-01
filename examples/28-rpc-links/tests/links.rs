mod common;

use ex_28_rpc_links::{bind, builder};
use ulo_runtime_tokio::Tokio;

#[tokio::test]
async fn the_handlers_answer_over_tcp() {
    common::the_handlers_answer(&format!("tcp://127.0.0.1:{}", common::free_port())).await;
}

#[tokio::test]
async fn a_scheme_with_no_link_is_refused_naming_it_and_the_ones_there_are() {
    let connected = builder().runtime(Tokio::current()).wire().expect("wires").connect().await.expect("connects");
    let error = bind(connected, "carrier-pigeon://coop:1").err().expect("refused");
    assert_eq!(error.to_string(), "no link for the scheme `carrier-pigeon`: expected tcp, nats, redis, mqtt, amqp or kafka");
}
