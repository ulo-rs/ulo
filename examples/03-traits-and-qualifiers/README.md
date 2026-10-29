# 03 traits and qualifiers

Two ways to bind more than one thing of a kind. `PgUserRepo as dyn UserRepo` binds a provider under a trait, and a qualifier (`Primary`, `Replica`) tells apart two instances of one type, read as `Dep<Db, Primary>`.
`Db("primary") @ Primary` binds a value under a qualifier in `#[module]`, and `Db @ Primary` in `exports` lets it leave the module; the `@` is the one reports print for a key.

```
cargo run -p ex-03-traits-and-qualifiers
```

The server listens on `PORT` (default 3000).

Read first:

- `src/lib.rs`: the trait binding, `DbModule` with its two qualified values, and the controller reading all three.
- `tests/qualifiers.rs`: each route answers from the instance its type or qualifier names; reading the unqualified `Dep<Db>` fails wiring as a missing dependency.
- `tests/binary.rs`: runs the compiled binary with `PORT` set to a free port, requests the address it prints and ends it with SIGTERM.

Next: [04 collections and optional](../04-collections-and-optional).
