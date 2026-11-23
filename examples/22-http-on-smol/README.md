# 22 http-on-smol

Example 01 on smol: `ulo-runtime-smol` for tasks and timers, `ulo-listener-smol` for sockets, hyper on
top. No tokio runtime is in the tree; `scripts/runtime_levels.py` checks it.

```bash
cargo run -p ex-22-http-on-smol
curl http://127.0.0.1:3000/
```

The server listens on `PORT` (default 3000); Ctrl-C closes it.

Read first:

- `src/main.rs`: `ulo_runtime_smol::run`, example 01's `main` with the runtime crate and the server
  swapped. It builds an executor, starts the application and serves until Ctrl-C or SIGTERM.
- `src/lib.rs`: the controller is example 01's; `server` is `ServerOn<SmolListener>`.
- `tests/hello.rs`: example 01's assertions, the phases written out on a `Smol` runtime with two
  worker threads, which `block_on` drives.

Previous: [21 http-in-axum](../21-http-in-axum).
