use ex_support::{Bin, send};

#[tokio::test]
async fn the_binary_answers_a_query_on_the_port_it_is_given_and_exits_clean_on_sigterm() {
    let mut bin = Bin::server(env!("CARGO_BIN_EXE_ex-31-graphql"), &[]);
    let body = r#"{"query":"{ greeting(name: \"bin\") }"}"#;
    let reply = send(
        bin.addr(),
        &format!(
            "POST /graphql HTTP/1.1\r\nhost: example\r\nconnection: close\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
            body.len()
        ),
    )
    .await;
    assert_eq!((reply.status, reply.body.as_str()), (200, r#"{"data":{"greeting":"hello bin"}}"#));
    let exit = bin.terminate();
    assert!(exit.status.success(), "{exit:?}");
    assert_eq!(exit.stderr, "");
}
