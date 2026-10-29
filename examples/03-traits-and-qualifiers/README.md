# 03 traits and qualifiers

Two ways to bind more than one thing of a kind. `PgUserRepo as dyn UserRepo` binds a provider under a trait, and a qualifier (`Primary`, `Replica`) tells apart two instances of one type, read as `Dep<Db, Primary>`.
`#[module]` has no spelling for a qualified binding, so `DbModule` is written against `ModuleDef`.

```
cargo run -p ex-03-traits-and-qualifiers
```

The server listens on `PORT` (default 3000).

Read first:

- `src/lib.rs`: the trait binding, `DbModule`, and the controller reading all three.
- `tests/qualifiers.rs`: each route answers from the instance its type or qualifier names; reading the unqualified `Dep<Db>` fails wiring as a missing dependency.
- `tests/binary.rs`: runs the compiled binary with `PORT` set to a free port, requests the address it prints and ends it with SIGTERM.

Next: [04 collections and optional](../04-collections-and-optional).
