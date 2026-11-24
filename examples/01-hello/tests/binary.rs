use ex_support::{Bin, get};

#[tokio::test]
async fn the_binary_serves_on_the_port_it_is_given_and_exits_clean_on_sigterm() {
    let mut bin = Bin::server(env!("CARGO_BIN_EXE_ex-01-hello"), &[]);
    let addr = bin.serving();
    assert_eq!(get(addr, "/").await.body, "Hello, ulo!");
    let exit = bin.terminate();
    assert!(exit.status.success(), "{exit:?}");
    assert!(exit.stdout.iter().any(|line| line.contains("listening") && line.contains(&addr.to_string())), "no `listening` line naming {addr}: {exit:?}");
    assert_eq!(exit.stderr, "");
}
