# 01 hello

One module, one controller, one route, served by `ulo_runtime_tokio::run`.

```bash
cargo run -p ex-01-hello
curl http://127.0.0.1:3000/
```

`GET /` answers `Hello, ulo!`. The server listens on `PORT` (default 3000); Ctrl-C closes it.

Read first:

- `src/main.rs`: `run` with the module and the server. It builds the tokio runtime, starts the
  application and serves until Ctrl-C or SIGTERM, then shuts it down.
- `src/lib.rs`: the controller and the module that mounts it.
- `tests/hello.rs`: the five phases `run` performs, written out on port 0 (`wire`, `connect`,
  `bind`, `listen`, `serve`), a request, then `close` and the `Shutdown` it returns. An
  application that sets a builder knob or reads the app between two phases writes them this way.
- `tests/binary.rs`: runs the compiled binary with `PORT` set to a free port, requests `/` once the
  port accepts and ends it with SIGTERM.

`ulo new` writes a larger starting point with configuration and health probes
(`crates/ulo-cli/src/templates/new`).

Next: [02 providers](../02-providers).
