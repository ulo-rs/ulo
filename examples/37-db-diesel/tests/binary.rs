use ex_support::{Bin, get};

#[tokio::test]
async fn the_binary_answers_a_query_and_exits_clean_on_sigterm() {
    let mut bin = Bin::server(env!("CARGO_BIN_EXE_ex-37-db-diesel"), &[]);
    let addr = bin.addr();
    assert_eq!(get(addr, "/count/1000").await.body, "1000");
    let exit = bin.terminate();
    assert!(exit.status.success(), "{exit:?}");
    assert_eq!(exit.stderr, "");
}
