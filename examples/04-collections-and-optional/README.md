# 04 collections and optional

Many providers contributed to one key, read as `Many<dyn Plugin>`, and a dependency that may be absent, read as `Option<Dep<T>>`.
`into dyn Plugin: [..]` contributes. Collection order is imports first, in the order written, then the module's own entries.

```
cargo run -p ex-04-collections-and-optional
```

The server listens on `PORT` (default 3000).

Read first:

- `src/lib.rs`: two modules contributing to `dyn Plugin`, and `Host` reading the collection and two optionals.
- `tests/collections.rs`: the order of the collection, and `Option` as `None` when nothing binds the key and `Some` when something does.
- `tests/binary.rs`: runs the compiled binary with `PORT` set to a free port, requests the address it prints and ends it with SIGTERM.

Next: [05 scopes](../05-scopes).
