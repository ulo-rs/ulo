# 02 providers

A provider is a type the container builds once and injects by `Dep<T>`. Both ways to declare one:

- struct style, `Counter`: every field is injected unless marked `#[injectable(default)]`, which is
  the type's own state;
- constructor style, `Greeter`: the parameters of `new` are injected, and `new` may be `async` and
  return a `Result`.

```
cargo run -p ex-02-providers
curl http://127.0.0.1:3000/    # Hello #1, then Hello #2
```

The server listens on `PORT` (default 3000).

Read first:

- `src/lib.rs`: both declarations and the module listing them.
- `tests/providers.rs`: two requests share one `Counter`; a variant whose constructor fails makes
- `tests/binary.rs`: runs the compiled binary with `PORT` set to a free port, requests the address it prints and ends it with SIGTERM.
  `connect` return `ConnectError::Construct` naming the key, the module and the constructor's error.

Next: [03 traits and qualifiers](../03-traits-and-qualifiers).
