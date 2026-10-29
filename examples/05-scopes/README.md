# 05 scopes

How long an instance lives: `singleton`, `execution` (once per request), `transient` (fresh at each injection point), and no scope written (`Auto`, a singleton for a provider).
Each request reads every scope twice, through two services, and prints the id of the instance each read reached.

```
cargo run -p ex-05-scopes
```

The server listens on `PORT` (default 3000).

Read first:

- `src/lib.rs`: one type per scope, and the two services that read them.
- `tests/scopes.rs`: which reads share an instance within a request and across requests; a singleton that reads a per-execution binding fails wiring with the dependency path.
- `tests/binary.rs`: runs the compiled binary with `PORT` set to a free port, requests the address it prints and ends it with SIGTERM.
- Compile-time refusals stay in trybuild: `hook_on_execution_type`, `hook_on_transient_type` and `ext_in_an_explicit_singleton` under `crates/ulo-codegen-tests/tests/ui`.

Next: [06 modules](../06-modules).
