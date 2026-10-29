use ex_support::Bin;
use ex_support::graphql::ready;
use ex_support::ws::hang_up;

#[tokio::test]
async fn the_binary_acknowledges_a_subscription_connection_and_exits_clean_on_sigterm() {
    let mut bin = Bin::server(env!("CARGO_BIN_EXE_ex-32-graphql-subscriptions"), &[]);
    hang_up(ready(bin.addr(), "/graphql/ws").await).await;
    let exit = bin.terminate();
    assert!(exit.status.success(), "{exit:?}");
    assert_eq!(exit.stderr, "");
}
