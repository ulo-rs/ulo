# 06 modules

Imports, exports and visibility. `BillingModule` imports `UsersModule` and reads `UserService`, which it exports. `PasswordHasher` is bound there and not exported, so no other module can read it.

```
cargo run -p ex-06-modules
```

The server listens on `PORT` (default 3000).

Read first:

- `src/lib.rs`: the two modules and what `UsersModule` exports.
- `tests/modules.rs`: the working application, and a module that reads the private binding failing wiring with the key, the reader and the reader's module.
- `tests/binary.rs`: runs the compiled binary with `PORT` set to a free port, requests the address it prints and ends it with SIGTERM.

Next: [07 configured modules](../07-configured-modules).
