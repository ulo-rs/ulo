//! A store module over a real Postgres in a container: the migration runs at start, a query goes
//! through the injected pool, and a lazy store's readiness follows the first ping.

#![cfg(feature = "integration")]

#[path = "support/postgres.rs"]
mod postgres;

use std::time::Duration;

use ex_35_db_sqlx::{builder, config, lazy_builder};
use ex_support::http::{Call, call};
use ex_support::{get, start};
use serde_json::{Value, json};
use ulo::Dep;
use ulo_config::{Env, Values};
use ulo_conformance_support::relay::Relay;
use ulo_db_sqlx::sqlx::{self, PgPool};

fn values(url: String) -> Values {
    // The migration pins one pooled connection and the migrator takes another.
    Values::new().set("database.url", url).set("database.max_connections", 5).set("database.acquire_timeout_secs", 1)
}

fn json(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or_else(|e| panic!("`{text}`: {e}"))
}

#[tokio::test(flavor = "multi_thread")]
async fn the_migration_runs_at_start_and_a_note_goes_through_the_injected_pool() {
    let db = postgres::Server::start("ulo-conf-ex35-pg").await;
    let config = config(Env::from_map::<&str, &str>([]), values(postgres::url(db.direct)));
    let server = start(builder(config)).await;

    let created = call(server.addr, Call::post("/notes").json(r#"{"title":"first"}"#)).await;
    assert_eq!((created.status, json(&created.body)), (201, json!({ "id": 1, "title": "first" })));
    assert_eq!(created.header("location"), Some("/notes/1"));
    assert_eq!(json(&get(server.addr, "/notes").await.body), json!([{ "id": 1, "title": "first" }]));

    let pool: Dep<PgPool> = server.handle.get().await.unwrap();
    let applied: i64 = sqlx::query_scalar("select count(*) from _sqlx_migrations where success").fetch_one(&*pool).await.unwrap();
    assert_eq!(applied, 1, "the one migration of the example");

    let ready = get(server.addr, "/health/ready").await;
    assert_eq!(ready.status, 200);
    assert_eq!(json(&ready.body), json!({ "status": "pass", "checks": { "postgres@database": [{ "status": "pass" }] } }));
    server.stop().await.unwrap();
}

/// `connect` returns with the database unreachable; readiness names the store and is 503 until a
/// ping gets through, and no probe body carries the password.
#[tokio::test(flavor = "multi_thread")]
async fn a_lazy_store_is_not_ready_until_its_first_ping_reaches_the_database() {
    let db = postgres::Server::start("ulo-conf-ex35-pg-lazy").await;
    let relay = Relay::start(db.direct).await;
    relay.cut_for(Duration::from_secs(3600)).await;
    let config = config(Env::from_map::<&str, &str>([]), values(postgres::url(relay.addr())));
    let server = start(lazy_builder(config)).await;

    let down = get(server.addr, "/health/ready").await;
    assert_eq!(down.status, 503);
    let body = json(&down.body);
    assert_eq!(body["status"], "fail");
    assert_eq!(body["checks"]["postgres@database"][0]["status"], "fail", "{body}");
    assert!(!down.body.contains(postgres::PASSWORD), "{}", down.body);

    relay.cut_for(Duration::ZERO).await;
    let up = get(server.addr, "/health/ready").await;
    assert_eq!((up.status, json(&up.body)["status"].as_str().map(str::to_owned)), (200, Some("pass".to_owned())));
    server.stop().await.unwrap();
}
