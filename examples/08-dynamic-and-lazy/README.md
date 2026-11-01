# 08 dynamic and lazy modules

A module built at runtime from a value (`DynamicModule`), and a module loaded after startup through `AppHandle::load`, which returns a `ModuleRef` limited to what that module sees.
Controllers cannot be loaded lazily: the routes are already bound.

```
cargo run -p ex-08-dynamic-and-lazy
```

The server listens on `PORT` (default 3000). `FEATURE` (default `alpha`) names the feature the runtime-built module is made from; `GET /feature` answers it.

Read first:

- `src/lib.rs`: `feature_module`, `ReportsModule`, and the log that records when `Reports` is built.
- `tests/lazy.rs`: nothing of `ReportsModule` is built before `load`; loading it twice builds once; a lazy module with a controller is refused with `LoadRefusal::Controllers` naming it.
- `tests/binary.rs`: runs the compiled binary with `PORT` set to a free port, requests the address it prints and ends it with SIGTERM.

Next: [09 lifecycle](../09-lifecycle).
