use ex_support::Bin;

#[test]
fn the_binary_without_urls_exits_1_naming_both_missing_keys() {
    let exit = Bin::spawn(env!("CARGO_BIN_EXE_ex-36-db-two-instances"), &[]).finish();
    assert_eq!(exit.status.code(), Some(1), "{exit:?}");
    assert!(exit.stderr.contains("primary.url") && exit.stderr.contains("replica.url"), "{}", exit.stderr);
    assert_eq!(exit.stdout, Vec::<String>::new());
}

#[cfg(feature = "integration")]
mod common;

#[cfg(feature = "integration")]
mod with_a_database {
    use ex_support::http::{Call, call};
    use ex_support::{Bin, get};

    use crate::common::{postgres, primary_url, replica_url, two_databases};

    #[tokio::test(flavor = "multi_thread")]
    async fn the_binary_serves_both_databases_and_exits_clean_on_sigterm() {
        let db = postgres::Server::start("ulo-conf-ex36-pg-bin").await;
        two_databases(&db).await;
        let (primary, replica) = (primary_url(db.direct), replica_url(db.direct));
        let envs = [("APP_PRIMARY__URL", primary.as_str()), ("APP_REPLICA__URL", replica.as_str()), ("APP_PRIMARY__MAX_CONNECTIONS", "5")];
        let mut bin = Bin::server(env!("CARGO_BIN_EXE_ex-36-db-two-instances"), &envs);
        let addr = bin.addr();
        let created = call(addr, Call::post("/notes").json(r#"{"title":"hello"}"#)).await;
        assert_eq!((created.status, created.body.as_str()), (201, r#"{"id":1,"title":"hello"}"#));
        assert_eq!(get(addr, "/replica/notes").await.body, "[]");
        let exit = bin.terminate();
        assert!(exit.status.success(), "{exit:?}");
        assert_eq!(exit.stderr, "");
    }
}
