# #170 — Declared metadata keeps every declaration

Merged 2026-08-22 into `master` from `feat/metadata-keeps-every-declaration`, commit [`7d5d88c`](https://github.com/ulo-rs/ulo/commit/7d5d88c9aca7ba640a79da285e1bc4b0a9c96527).

An impl block's entry survives a handler declaring the same type, and a reader chooses which it wants.

Before this, the handler's entry replaced the block's outright:

```rust
#[routes]
#[set_metadata(Roles(vec!["authenticated"]))]
impl Admin {
    #[get("/panel")]
    #[set_metadata(Roles(vec!["admin"]))]   // the block's requirement is gone
    fn panel(&self) -> ToniBody { … }
}
```

Restating `authenticated` on the handler was the only way to keep it, and that drifts the moment the controller's rule changes.

```rust
metadata.get::<Roles>()      // the handler's — unchanged behaviour
metadata.get_all::<Roles>()  // both, block first
```

`get` stays the common case, most metadata being a setting where the nearer declaration is the one that applies. `get_all` is for declarations that add up.

## It combines nothing, deliberately

Nest's `getAllAndMerge` merges generically on the strength of arrays concatenating and objects spreading. Rust has no such operation for an arbitrary `T`, and inventing one guesses wrong on the first type that is a setting rather than a set — a `RateLimit` spread into a hybrid nobody declared.

Knowing how to combine two values lives where their type is defined, which is also where the reader that wants them combined lives. So the storage keeps both and stops there.

## Surface

`Metadata::insert` records rather than replaces and returns nothing; it is macro-called. `Metadata::get` answers the same thing it did and gains a `Clone` bound, every declared type being one already. `TypeMap` is untouched — `RpcCallInfo` wants replacement and keeps it.

## Tests

Both readers pinned against the two-level overlay. Falsified by restoring replacement: `get_all` collapses to the handler's entry alone while `get` is unaffected, which is the half that would otherwise go unnoticed.

379 integration tests pass.
