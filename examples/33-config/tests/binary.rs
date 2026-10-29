use ex_support::{Bin, get};
use serde_json::Value;

#[tokio::test]
async fn the_binary_serves_the_port_it_is_given_through_the_explicit_layer() {
    let mut bin = Bin::server(env!("CARGO_BIN_EXE_ex-33-config"), &[("APP_GREETING", "hello from the shell")]);
    let addr = bin.addr();
    let body: Value = serde_json::from_str(&get(addr, "/settings").await.body).unwrap();
    assert_eq!((body["host"].as_str(), body["port"].as_u64()), (Some("127.0.0.1"), Some(u64::from(addr.port()))));
    assert_eq!(body["greeting"], "hello from the shell", "the process environment `main` hands in overrides the .env file");
    let exit = bin.terminate();
    assert!(exit.status.success(), "{exit:?}");
    assert_eq!(exit.stderr, "");
}
