use ex_39_one_app::grpc::pb::Note;
use ex_39_one_app::grpc::pb::notes_client::NotesClient;
use ex_support::rpc::bounded;
use ex_support::ws::{connect, hang_up, next, send};
use ex_support::{Bin, get};
use serde_json::json;
use tonic::transport::Endpoint;
use ulo_rpc::RpcClient;
use ulo_rpc_tcp::Tcp;
use ulo_runtime_tokio::Tokio;

fn free_port() -> String {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port().to_string()
}

fn address(line: &str, scheme: &str) -> std::net::SocketAddr {
    line.strip_prefix(&format!("listening on {scheme}://")).unwrap_or_else(|| panic!("`{line}` is not a {scheme} line")).parse().unwrap()
}

// `terminate` blocks the calling thread until the binary exits; the gRPC client's connection task
// has to keep running meanwhile to answer the server's GOAWAY.
#[tokio::test(flavor = "multi_thread")]
async fn the_binary_listens_on_four_ports_in_bind_order_answers_on_each_and_exits_clean_on_sigterm() {
    let ports = [free_port(), free_port(), free_port()];
    // `APP_PORTS__HTTP` loses to `PORT`, which `main` sets in code, the last layer.
    let envs = [("APP_PORTS__HTTP", "1"), ("APP_PORTS__WS", ports[0].as_str()), ("APP_PORTS__RPC", ports[1].as_str()), ("APP_PORTS__GRPC", ports[2].as_str())];
    let mut bin = Bin::server(env!("CARGO_BIN_EXE_ex-39-one-app"), &envs);
    let http = bin.addr();
    let [ws, rpc, grpc] = ["ws", "tcp", "grpc"].map(|scheme| address(&bin.next_line(), scheme));
    assert_eq!([ws.port(), rpc.port(), grpc.port()].map(|p| p.to_string()), ports, "each transport listens on the port the environment names");
    assert_eq!(bin.next_line(), "store open");

    assert_eq!(get(http, "/health/ready").await.status, 200);
    assert_eq!(get(http, "/greeting").await.body, "hello from the defaults");
    let mut socket = connect(ws, "/notes", &[]).await;
    send(&mut socket, json!({ "event": "add", "id": 1, "data": "ws" })).await;
    assert_eq!(next(&mut socket).await, json!({ "id": 1, "data": 1 }));
    hang_up(socket).await;
    let client = RpcClient::new(Tcp::new(rpc), std::sync::Arc::new(Tokio::current())).unwrap();
    assert_eq!(bounded(client.request::<_, u32>("notes.add", "rpc")).await.unwrap(), 2);
    drop(client);
    let channel = Endpoint::from_shared(format!("http://{grpc}")).unwrap().connect().await.unwrap();
    assert_eq!(NotesClient::new(channel).add(Note { text: "grpc".into() }).await.unwrap().into_inner().n, 3);
    let notes: Vec<String> = serde_json::from_str(&get(http, "/notes").await.body).unwrap();
    assert_eq!(notes, ["ws: ws", "rpc: rpc", "grpc: grpc"]);

    let exit = bin.terminate();
    assert!(exit.status.success(), "{exit:?}");
    assert_eq!(exit.stdout.last().map(String::as_str), Some("closed on SIGTERM"));
    assert_eq!(exit.stderr, "");
}
