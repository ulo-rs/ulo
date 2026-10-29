# 09 lifecycle

Every hook kind, in the order it runs, and a readiness check with retries and a backoff.
A trait hook is an `impl OnModuleInit` (and the four others) on an injectable type. Closure hooks, `.ready(..)` and a module's own `on_init` hang off `ModuleDef`, so `LinkModule` is written against it. `main` connects, closes and prints the journal.

```
cargo run -p ex-09-lifecycle
```

Read first:

- `src/lib.rs`: `Cache` (all five hooks), `LinkModule` (readiness, `on_destroy`, module `on_init`).
- `tests/lifecycle.rs`: the journal in exact order, startup and close; a check that never passes fails `connect` with `ConnectError::Readiness` naming the key, the attempts made and the last attempt's error. The backoff runs on tokio's paused test clock, so the test waits for nothing.
- `tests/binary.rs`: runs the compiled binary to its end and checks what it prints.

Next: [10 shutdown](../10-shutdown).
