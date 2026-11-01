//! Two databases on one server, read through two qualified pools: each repository reads its own,
//! and the two health checks are named apart so a replica outage is visible as the replica's.

#![cfg(feature = "integration")]

mod common;

use common::{postgres, primary_url, replica_url, two_databases};
use ex_36_db_two_instances::{PrimaryNotes, ReplicaNotes, builder, config};
use ex_support::http::{Call, call};
use ex_support::{get, start};
use serde_json::{Value, json};
use std::time::Duration;
use ulo::Dep;
use ulo_config::{Env, Values};
use ulo_conformance_support::relay::Relay;
use ulo_db_sqlx::sqlx;

fn json(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or_else(|e| panic!("`{text}`: {e}"))
}

#[tokio::test(flavor = "multi_thread")]
async fn each_repository_reads_its_own_database_and_a_replica_outage_names_the_replica() {
    let server = postgres::Server::start("ulo-conf-ex36-pg").await;
    two_databases(&server).await;
    let (primary_relay, replica_relay) = (Relay::start(server.direct).await, Relay::start(server.direct).await);
    let values = Values::new()
        .set("primary.url", primary_url(primary_relay.addr()))
        .set("primary.max_connections", 5)
        .set("primary.acquire_timeout_secs", 1)
        .set("replica.url", replica_url(replica_relay.addr()))
        .set("replica.acquire_timeout_secs", 1);
    let app = start(builder(config(Env::from_map::<&str, &str>([]), values))).await;

    let created = call(app.addr, Call::post("/notes").json(r#"{"title":"on the primary"}"#)).await;
    assert_eq!((created.status, json(&created.body)), (201, json!({ "id": 1, "title": "on the primary" })));
    assert_eq!(json(&get(app.addr, "/primary/notes").await.body), json!([{ "id": 1, "title": "on the primary" }]));
    assert_eq!(json(&get(app.addr, "/replica/notes").await.body), json!([]), "the replica is another database: the write is not there");

    let replica: Dep<ReplicaNotes> = app.handle.get().await.unwrap();
    let replica = replica.pool();
    sqlx::query("insert into notes (title) values ('on the replica')").execute(replica).await.unwrap();
    assert_eq!(json(&get(app.addr, "/replica/notes").await.body), json!([{ "id": 1, "title": "on the replica" }]));
    assert_eq!(json(&get(app.addr, "/primary/notes").await.body).as_array().map(Vec::len), Some(1));
    let primary: Dep<PrimaryNotes> = app.handle.get().await.unwrap();
    let primary = primary.pool();
    let names: Vec<String> = sqlx::query_scalar("select current_database()").fetch_all(primary).await.unwrap();
    let replica_name: Vec<String> = sqlx::query_scalar("select current_database()").fetch_all(replica).await.unwrap();
    assert_eq!((names, replica_name), (vec!["postgres".to_owned()], vec!["replica".to_owned()]));

    let ready = get(app.addr, "/health/ready").await;
    assert_eq!(ready.status, 200);
    let checks = json(&ready.body)["checks"].clone();
    let mut named: Vec<&String> = checks.as_object().unwrap().keys().collect();
    named.sort();
    assert_eq!(named, ["postgres@primary @ Primary", "postgres@replica @ Replica"]);

    replica_relay.cut_for(Duration::from_secs(3600)).await;
    let down = get(app.addr, "/health/ready").await;
    assert_eq!(down.status, 503);
    let body = json(&down.body);
    assert_eq!(body["checks"]["postgres@replica @ Replica"][0]["status"], "fail", "{body}");
    assert_eq!(body["checks"]["postgres@primary @ Primary"][0]["status"], "pass", "{body}");
    assert_eq!(get(app.addr, "/primary/notes").await.status, 200, "the primary still answers");

    replica_relay.cut_for(Duration::ZERO).await;
    assert_eq!(get(app.addr, "/health/ready").await.status, 200);
    app.stop().await.unwrap();
}
