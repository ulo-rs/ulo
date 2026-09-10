# #183 — fix(db): check the registered connection instead of opening a second one

Merged 2026-08-24 into `master` from `fix/health-indicators-share-the-connection`, commit [`971c632`](https://github.com/ulo-rs/ulo/commit/971c632623ba0d23e0bb354b97887a0e1e2b1e14).

Every `*HealthIndicatorFactory` opened its own pool or client from the same URL the connection factory had already used. An application with the `health` feature therefore ran two connections per database, and paid two of every failure the connection path can produce — two dials, two chances to leak credentials, two things to configure once connect timeouts exist.

Each factory now declares the connection's token as a dependency and holds its provider. The indicator is built when one is resolved rather than when the factory runs: the connection may have failed, and it reports that from its own `on_module_init`, so resolving eagerly here would reach the absent connection first and panic on it.

Each crate is left with exactly one call that opens a connection.

### Components

- **toni-seaorm, toni-redis, toni-sqlx, toni-mongodb, toni-diesel** — the health factory depends on the connection token and resolves through it; the connection it used to open, along with that path's carried-failure reporting and redaction, goes with it.

### Verification

Each crate's `health` integration suite exercises the full path — the indicator is injected into a controller, a `/health` endpoint runs it through `HealthCheckService`, and the response is asserted against a real database in a container. All five pass. Pointing one crate's dependency at a token nothing registers makes its suite fail, so they discriminate on the wiring rather than merely compiling.

Those suites stay behind the `integration` feature and out of CI, which needs no Docker; the hermetic startup cases still run there.
