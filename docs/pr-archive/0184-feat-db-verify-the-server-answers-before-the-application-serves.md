# #184 — feat(db)!: verify the server answers before the application serves

Merged 2026-08-24 into `master` from `feat/db-startup-checks`, commit [`3ca9932`](https://github.com/ulo-rs/ulo/commit/3ca9932855f980585d375b3e607a59933e074f54).

The database integrations disagreed about when an unreachable server was discovered, because each inherited whatever its driver does while building a pool:

| | construction did | an unreachable server surfaced |
| --- | --- | --- |
| seaorm | `Database::connect` | at startup, after sqlx's 30-second acquire timeout |
| redis | `ConnectionManager::new` | at startup, after six retries with the driver's backoff |
| sqlx | `Pool::connect` | at startup, after a 30-second acquire timeout |
| mongodb | `Client::with_options` | on the first query |
| diesel | deadpool `build()` | on the first query |

The same wrong connection string failed a deployment for three of them and returned 500s to users for the other two. Where it did fail, thirty seconds outlasts many readiness deadlines, so the container is killed before the diagnosis arrives.

Every integration now configures its pool or client without touching the network, hands the per-attempt bound to its driver, and probes on the check's schedule from `on_module_init`.

```rust
SeaOrmModule::for_root(url)                                       // checked, with the defaults
RedisModule::for_root(url).without_startup_check()
SqlxModule::postgres(url).with_startup_check(StartupCheck::default().attempts(5))
```

Defaults are three attempts, two seconds apart, five seconds each. On by default because the application whose readiness probe does not report its database is the one that most needs the failure surfaced; turning it off is then a decision visible in the code that turns it off.

### No crate acquires a runtime

Bounding an attempt stays with the driver, which already knows how. Only the gap between attempts is scheduled by the framework, and `StartupCheck::run` takes the sleep it uses as an argument — so core depends on no runtime and constructs no timer, and a probe that answers first time sleeps zero times.

The integrations pass `futures_timer::Delay::new`. It has no dependencies of its own, and it was checked to fire correctly both under a tokio runtime and with no runtime at all. Its thread is spawned on first use, so only a startup that actually retries pays for it.

Net dependency change across the workspace is one crate. No crate here gains a direct tokio dependency.

### Components

- **toni** — `StartupCheck` (attempts, gap, per-attempt bound, and a probe runner) and `CheckedModule`, which carries the policy alongside the module so constructors need no `_with` twin per combination.
- **toni-seaorm, toni-redis, toni-sqlx, toni-mongodb, toni-diesel** — lazy construction, the driver given the per-attempt bound, the probe in `on_module_init`, and `for_root` and its siblings returning `CheckedModule`. Drivers that retry internally are switched off, so one schedule applies.
- **toni-prisma** — cannot participate, and says so: its constructor takes a closure returning the generated client by value, and there is no operation to call on an arbitrary type to see whether it works.
- **docs** — ADR 0026, including the two designs that were rejected and why.

### Measurements

Against an unreachable server, with two attempts 50ms apart bounded at 400ms each:

| | before | after |
| --- | --- | --- |
| redis | startup failed after 9.46s, bounded by no available setting | startup fails in 0.06s |
| seaorm, sqlx, mongodb | startup failed after the driver's 30s timeout | startup fails in 0.86s |
| diesel | **startup succeeded with no database present**; the first query failed | startup fails in 0.05s |

Each integration's unreachable case bounds elapsed time from both sides: an upper bound because the defect being replaced was a wait rather than a wrong answer, and a lower one because without the retry the check would fail on the first refused connection.

All five `health` suites still pass against real databases in containers, which is what confirms the lazy construction and the probe reach the same connection.

### Breaking

`for_root` and its siblings return `CheckedModule` rather than `DynamicModule`; `imports: [...]` is unaffected, but a binding annotated as `DynamicModule` needs updating. mongodb and diesel applications that started against an absent database no longer start — `without_startup_check()` restores that where it was wanted.
