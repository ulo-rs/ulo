# 13 testing

`TestApp` builds the production module graph with parts replaced: `override_value`, `override_factory`, `.in_module::<M>()` to scope one, `replace_module`, and `.timer(..)` / `.runtime(..)` for the clock.
An override that matches no binding, or two, fails wiring, so a stale mock cannot pass.

```
cargo test -p ex-13-testing
```

The server listens on `PORT` (default 3000).

Read first:

- `src/lib.rs`: the production app: a mailer module, two private `Ledger`s, a factory-built `Stamp`, and an `Uptime` that reads the clock.
- `tests/testing.rs`: each kind of replacement answering through HTTP; a paused tokio clock setting what `Uptime` reads; the two refusals, with the key, the declaring line and the modules named.
- `tests/binary.rs`: runs the compiled binary with `PORT` set to a free port, requests the address it prints and ends it with SIGTERM.

Next: [14 without macros](../14-without-macros).
