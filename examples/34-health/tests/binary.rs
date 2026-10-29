use ex_support::{Bin, get};

#[tokio::test]
async fn the_binary_is_ready_once_it_has_warmed_the_cache_and_exits_clean_on_sigterm() {
    let grpc = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port().to_string();
    let mut bin = Bin::server(env!("CARGO_BIN_EXE_ex-34-health"), &[("GRPC_PORT", grpc.as_str())]);
    let addr = bin.addr();
    assert_eq!(bin.next_line(), format!("listening on http://127.0.0.1:{grpc}"), "the gRPC server, on the port it is given");
    assert_eq!(bin.next_line(), "cache warm");
    assert_eq!(get(addr, "/health/ready").await.status, 200);
    assert_eq!(get(addr, "/health/live").await.status, 200);
    let exit = bin.terminate();
    assert!(exit.status.success(), "{exit:?}");
    assert_eq!(exit.stderr, "");
}
