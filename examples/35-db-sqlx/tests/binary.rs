use ex_support::Bin;

/// Without `APP_DATABASE__URL` the binary stops at `wire()` and prints the report on stderr.
#[test]
fn the_binary_without_a_database_url_exits_1_naming_the_missing_key() {
    let exit = Bin::spawn(env!("CARGO_BIN_EXE_ex-35-db-sqlx"), &[]).finish();
    assert_eq!(exit.status.code(), Some(1), "{exit:?}");
    assert!(exit.stderr.contains("database.url"), "{}", exit.stderr);
    assert_eq!(exit.stdout, Vec::<String>::new());
}

#[cfg(feature = "integration")]
#[path = "support/postgres.rs"]
mod postgres;

#[cfg(feature = "integration")]
mod with_a_database {
    use ex_support::http::{Call, call};
    use ex_support::{Bin, get};

    use crate::postgres;

    #[tokio::test(flavor = "multi_thread")]
    async fn the_binary_migrates_serves_notes_and_exits_clean_on_sigterm() {
        let db = postgres::Server::start("ulo-conf-ex35-pg-bin").await;
        let url = postgres::url(db.direct);
        let mut bin = Bin::server(env!("CARGO_BIN_EXE_ex-35-db-sqlx"), &[("APP_DATABASE__URL", url.as_str())]);
        let addr = bin.addr();
        let created = call(addr, Call::post("/notes").json(r#"{"title":"hello"}"#)).await;
        assert_eq!((created.status, created.body.as_str()), (201, r#"{"id":1,"title":"hello"}"#));
        assert_eq!(get(addr, "/health/ready").await.status, 200);
        let exit = bin.terminate();
        assert!(exit.status.success(), "{exit:?}");
        assert_eq!(exit.stderr, "");
    }
}
