use ex_support::Bin;
use ex_support::http::{Call, call};

#[tokio::test]
async fn the_binary_serves_on_the_port_it_is_given_and_exits_clean_on_sigterm() {
    let mut bin = Bin::server(env!("CARGO_BIN_EXE_ex-19-http-uploads"), &[]);
    let addr = bin.addr();
    assert_eq!(call(addr, Call::get("/files/none")).await.status, 404);
    let exit = bin.terminate();
    assert!(exit.status.success(), "{exit:?}");
    assert_eq!(exit.stderr, "");
}
