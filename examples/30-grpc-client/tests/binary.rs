use std::time::Duration;

use ex_30_grpc_client::pb::Probe;
use ex_30_grpc_client::pb::front_client::FrontClient;
use ex_30_grpc_client::{Calls, callee_builder};
use ex_support::{Bin, serve};
use tonic::Request;
use tonic::transport::Endpoint;
use ulo_grpc::Server;
use ulo_runtime_tokio::Tokio;

// `terminate` blocks the calling thread until the binary exits. The client's connection task has to
// keep running meanwhile to answer the server's GOAWAY, which a current-thread runtime cannot do.
#[tokio::test(flavor = "multi_thread")]
async fn the_binary_serves_on_the_port_it_is_given_and_exits_clean_on_sigterm() {
    let app = callee_builder(Calls::default())
        .runtime(Tokio::current())
        .wire()
        .expect("the callee wires")
        .connect()
        .await
        .expect("the callee connects")
        .bind(Server::new("127.0.0.1:0"))
        .listen()
        .await
        .expect("the callee listens");
    let callee = serve(app);

    let origin = format!("http://{}", callee.addr);
    let mut bin = Bin::server(env!("CARGO_BIN_EXE_ex-30-grpc-client"), &[("CALLEE", origin.as_str())]);
    let addr = bin.addr();
    let channel = Endpoint::from_shared(format!("http://{addr}")).expect("an endpoint").connect().await.expect("the client connects");
    let mut request = Request::new(Probe {});
    request.set_timeout(Duration::from_secs(5));
    let left = FrontClient::new(channel).ask(request).await.expect("Ask answers").into_inner();
    assert!(left.has_deadline && (1..=5000).contains(&left.millis), "{left:?}");
    let exit = bin.terminate();
    assert!(exit.status.success(), "{exit:?}");
    assert_eq!(exit.stderr, "");
    callee.stop().await.expect("the callee closes");
}
