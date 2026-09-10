# #55 — feat(terminus): add health checks module with database indicators

Merged 2026-04-24 into `master` from `feat/terminus`, commit [`c302154`](https://github.com/ulo-rs/ulo/commit/c302154a245f0ef9f9298a649675a78f9fc2f477).

Introduces `toni-terminus`, a health checks module modelled on NestJS
Terminus, and wires database health indicators into each existing
integration crate.

## toni-terminus

- `HealthCheckService::check()` runs indicators concurrently and returns
  HTTP 200 / 503 with a NestJS-compatible JSON shape
- Built-in indicators: `HttpHealthIndicator` (`ping_check`,
  `response_check`), `MemoryHealthIndicator`, `DiskHealthIndicator`
- Optional per-check timeout via `.timeout(Duration).await` (requires
  the `timeout` feature)
- Custom indicators: implement the `HealthIndicator` trait

## Database health indicators

Each crate gains an optional `health` feature. Enabling it registers
an additional indicator provider alongside the main connection, using
its own separate connection so probes don't compete with app traffic.

| Crate | Type | Probe |
|---|---|---|
| toni-redis | `RedisHealthIndicator` | `PING` |
| toni-mongodb | `MongoHealthIndicator` | `{ ping: 1 }` |
| toni-seaorm | `SeaOrmHealthIndicator` | `DatabaseConnection::ping()` |
| toni-sqlx | `SqlxHealthIndicator<DB>` | `SELECT 1` |
| toni-diesel | `PgHealthIndicator` / `MySqlHealthIndicator` | `SELECT 1` |

## Fixes

- `Path<T>` extractor now handles scalar types (`Path<String>`,
  `Path<i32>`) by falling back from map deserialization to the single
  bare value when the map attempt fails
- `toni-diesel` postgres feature no longer requires a native libpq
  system library (`diesel/postgres_backend` instead of `diesel/postgres`)
- `toni-redis-broadcast` integration tests now use `#![cfg(feature = "integration")]`
  consistent with the other database crate tests (removes per-test `#[ignore]`)
