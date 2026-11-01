use ex_29_grpc_service::pb::GetItemRequest;
use ex_29_grpc_service::pb::catalog_client::CatalogClient;
use ex_support::Bin;
use tonic::Request;
use tonic::transport::Endpoint;

// `terminate` blocks the calling thread until the binary exits. The client's connection task has to
// keep running meanwhile to answer the server's GOAWAY, which a current-thread runtime cannot do.
#[tokio::test(flavor = "multi_thread")]
async fn the_binary_serves_on_the_port_it_is_given_and_exits_clean_on_sigterm() {
    let mut bin = Bin::server(env!("CARGO_BIN_EXE_ex-29-grpc-service"), &[]);
    let addr = bin.addr();
    let channel = Endpoint::from_shared(format!("http://{addr}")).expect("an endpoint").connect().await.expect("the client connects");
    let mut request = Request::new(GetItemRequest { id: "c3".into() });
    request.metadata_mut().insert("authorization", ex_29_grpc_service::TOKEN.parse().expect("ascii metadata"));
    let mut client = CatalogClient::new(channel);
    let item = client.get_item(request).await.expect("GetItem answers").into_inner();
    assert_eq!(item.name, "crucible");
    let exit = bin.terminate();
    assert!(exit.status.success(), "{exit:?}");
    assert_eq!(exit.stderr, "");
}
