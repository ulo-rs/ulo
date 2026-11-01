# 07 configured modules

A global module, a module that takes a value (`StoreModule::for_root(url)`), and one module imported twice under two qualifiers with `.keyed::<Q>()`.
A module is identified by its type and its value: the same value imported twice is one module, and two values of one type are two.

```
cargo run -p ex-07-configured-modules
```

The server listens on `PORT` (default 3000).

Read first:

- `src/lib.rs`: `ConfigModule` (global), `StoreModule`, and the keyed imports.
- `tests/configured.rs`: each keyed instance answers with its own configuration and the identity `StoreModule @ Primary`; two unkeyed configurations export one key twice and wiring names both modules.
- `tests/binary.rs`: runs the compiled binary with `PORT` set to a free port, requests the address it prints and ends it with SIGTERM.

Next: [08 dynamic and lazy](../08-dynamic-and-lazy).
