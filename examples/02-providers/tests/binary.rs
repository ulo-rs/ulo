use ex_support::{Bin, get};

#[tokio::test]
async fn the_binary_serves_on_the_port_it_is_given_and_exits_clean_on_sigterm() {
    let mut bin = Bin::server(env!("CARGO_BIN_EXE_ex-02-providers"), &[]);
    let addr = bin.addr();
    assert_eq!(get(addr, "/").await.body, "Hello #1");
    let exit = bin.terminate();
    assert!(exit.status.success(), "{exit:?}");
    assert_eq!(exit.stderr, "");
}
