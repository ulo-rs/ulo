#![allow(dead_code)]

#[path = "../../../35-db-sqlx/tests/support/postgres.rs"]
pub mod postgres;

use std::net::SocketAddr;

use ulo_db_sqlx::sqlx::{self, PgPool};

/// One server, two databases: `postgres` is the primary, `replica` is made here with the schema a
/// replica would have received by replication.
pub async fn two_databases(server: &postgres::Server) {
    let admin = PgPool::connect(&postgres::url(server.direct)).await.unwrap();
    sqlx::query("create database replica").execute(&admin).await.unwrap();
    let replica = PgPool::connect(&format!("{}/replica", postgres::url(server.direct).trim_end_matches("/postgres"))).await.unwrap();
    sqlx::query("create table notes (id bigserial primary key, title text not null)").execute(&replica).await.unwrap();
}

pub fn primary_url(addr: SocketAddr) -> String {
    postgres::url(addr)
}

pub fn replica_url(addr: SocketAddr) -> String {
    format!("postgres://postgres:{}@{addr}/replica", postgres::PASSWORD)
}
