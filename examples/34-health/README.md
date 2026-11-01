# 34 health

Probes for an orchestrator. `HealthModule` collects every `dyn Check` that any module contributes; `HealthHttp` serves `/health/live`, `/health/ready` and `/health/started` as `application/health+json`, and `HealthGrpc` writes the `grpc.health.v1` overall status from the same readiness. Here `CacheModule` contributes a check named `cache` that is down until the application warms the cache.

```
cargo test -p ex-34-health
PORT=7000 cargo run -p ex-34-health
curl -i http://127.0.0.1:7000/health/ready
```

`ready` answers 503 with `checks.cache` failing until `main` has warmed the cache after `listen`, then 200. When the close begins, readiness turns false at once and the app waits `LAG` (2 s) before it stops accepting, so a load balancer stops sending first; `live` stays 200 throughout. A check that does not answer within its bound counts as down.

Read first:

- `src/lib.rs`: the check, the module that contributes it, and the three imports.
- `src/main.rs`: the warm-up after `listen`.
- `tests/probes.rs`: 503 naming the cold check, 200 once warm; the close on a scripted clock, with `ready` 503 naming `lifecycle` while the listener is still open.
- `tests/grpc.rs`: the same sequence read through `grpc.health.v1`.
- `tests/binary.rs`: runs the compiled binary with `PORT` and `GRPC_PORT` set and ends it with SIGTERM.

Nothing answers a probe before `listen`, so readiness before it is read in process, through `Dep<Readiness>`.

`ulo new` imports `HealthModule` and the probes of the transport it is given (`crates/ulo-cli/src/templates/new`).

Next: [35 db-sqlx](../35-db-sqlx).
