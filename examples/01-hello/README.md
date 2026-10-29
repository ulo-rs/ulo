# 01 hello

One module, one controller, one route, and the five phases that start an application:
`wire`, `connect`, `bind`, `listen`, `serve`.

```
cargo run -p ex-01-hello
curl http://127.0.0.1:3000/
```

`GET /` answers `Hello, ulo!`. The server listens on `PORT` (default 3000); Ctrl-C closes it.

Read first:

- `src/main.rs`: the phases in order, and the runtime named with `.runtime(..)`.
- `src/lib.rs`: the controller and the module that mounts it.
- `tests/hello.rs`: the same phases on port 0, a request, then `close` and the `Shutdown` it returns.
- `tests/binary.rs`: runs the compiled binary with `PORT` set to a free port, requests the address it prints and ends it with SIGTERM.

`ulo new` writes a larger starting point with configuration and health probes
(`crates/ulo-cli/src/templates/new`).

Next: [02 providers](../02-providers).
