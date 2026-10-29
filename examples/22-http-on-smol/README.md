# 22 http-on-smol

Example 01 on smol: `ulo-runtime-smol` for tasks and timers, `ulo-listener-smol` for sockets, hyper on
top. No tokio runtime is in the tree; `scripts/runtime_levels.py` checks it.

```
cargo run -p ex-22-http-on-smol
curl http://127.0.0.1:3000/
```

The server listens on `PORT` (default 3000); Ctrl-C closes it.

Read first:

- `src/main.rs`: the runtime is `Smol::new(executor)` and `block_on` drives the app. The SIGINT/SIGTERM future is
  written here with `async-signal`, since only the tokio crate ships one.
- `src/lib.rs`: the controller is example 01's; `server` is `ServerOn<SmolListener>`.
- `tests/hello.rs`: example 01's assertions, on a smol runtime with two worker threads.

Previous: [21 http-in-axum](../21-http-in-axum).
