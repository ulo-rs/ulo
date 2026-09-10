# #182 — fix(db): report connection failures without leaking the connection string

Merged 2026-08-24 into `master` from `fix/db-modules-report-connection-failure`, commit [`fa64101`](https://github.com/ulo-rs/ulo/commit/fa64101a76d3c186781291e77011564c3312c76d).

All six database modules panicked from inside `ProviderFactory::build` when they could not reach their server or parse their URL — fourteen call sites across the connection providers and the health indicators.

`build` returns the instance directly and has no way to fail, which is the boundary ADR 0025 recorded. It does not need one: the failure is carried into the provider and returned from `on_module_init`, which the scanner already calls for every non-request-scoped provider. A caller of `create` now gets `StartupError::HookFailed` naming the module, including the name a `for_root_named` connection was registered under.

### The connection string was reaching the logs

The panic messages quoted the URL, and credentials live in a URL. Toni is not the only source: the drivers quote it too. sea-orm renders `The connection string 'postgres://someone:secret@host/db' has no supporting driver`, so passing the driver's message through verbatim leaks the password just as the panic did. Messages pass through a redaction that masks it, which is why the driver error is reformatted rather than boxed.

### What does not change

When a failure is discovered. seaorm, redis and sqlx still dial while building; mongodb and diesel still construct without touching the network. Making that uniform is a behaviour decision and is deferred: it would stop applications booting that boot today.

Two things found while testing, worth knowing before that decision:

- **The eager modules do not fail fast.** sqlx and sea-orm retry a refused connection until their 30-second `acquire_timeout`, so an unreachable database hangs startup for half a minute before reporting. A readiness deadline shorter than that kills the container first.
- **The health feature opens a second connection.** Every `*HealthIndicatorFactory` builds its own pool or client rather than using the registered one, so an application with `health` on runs twice the connections. Removing the duplication changes the DI shape and belongs on its own.

### Components

- **toni-seaorm, toni-redis, toni-sqlx, toni-mongodb, toni-diesel** — connection and health providers carry their failure and report it from `on_module_init`; a `redact` module masks the password in any message that quotes the URL.
- **tests** — a hermetic case per module: the failure must reach the caller as `HookFailed`, name its module, and carry no credentials. Forgetting `on_module_init` on one provider is what these catch, and the compiler cannot.
- **ci** — the test job runs them; they contact no server.
