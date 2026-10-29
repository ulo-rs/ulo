# 36 db-two-instances

Two databases of one kind. One store module is read from two configuration sections, `primary` and `replica`, and keyed twice with `.keyed::<Primary>()` and `.keyed::<Replica>()`: the application injects `Dep<PgPool, Primary>` and `Dep<PgPool, Replica>`, and the two health checks are named apart, `postgres@primary @ Primary` and `postgres@replica @ Replica`. Only the primary migrates.

```
docker run --rm --name pg -p 5432:5432 -e POSTGRES_PASSWORD=pw postgres:18     # in a terminal of its own
docker exec pg psql -U postgres -c 'create database replica'
docker exec -i pg psql -U postgres -d replica < examples/36-db-two-instances/migrations/0001_notes.sql
APP_PRIMARY__URL=postgres://postgres:pw@127.0.0.1/postgres APP_PRIMARY__MAX_CONNECTIONS=5 \
APP_REPLICA__URL=postgres://postgres:pw@127.0.0.1/replica PORT=7000 cargo run -p ex-36-db-two-instances
```

The `replica` database stands in for a replica, so its table is created by hand where replication would bring it. `POST /notes` writes to the primary; `GET /primary/notes` and `GET /replica/notes` each read their own database. A replica outage turns `/health/ready` 503 with the replica's check failing and the primary's passing.

```
cargo test -p ex-36-db-two-instances
cargo test -p ex-36-db-two-instances --features integration  # one Postgres container, two databases; needs Docker
```

Read first:

- `src/lib.rs`: the two imports and the two repositories.
- `tests/instances.rs`: separate data per database, the two check names, an outage of one.
- `tests/wiring.rs`: one report for both sections.
- `tests/binary.rs`: the binary without URLs names both keys; with a database it serves both.

The test server comes from `35-db-sqlx/tests/support/postgres.rs`. The example needs Rust 1.94, SQLx's floor.

Next: [37 db-diesel](../37-db-diesel).
