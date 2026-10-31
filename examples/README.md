# ulo examples

Forty-one applications, one concept each, numbered in the order they are best read. The root
[README](../README.md) lists them with one line each; every directory holds its own `README.md`
saying what it shows and how to run it.

```bash
cargo run -p ex-01-hello
cargo test -p ex-15-http-requests
```

The package name is the directory name prefixed with `ex-`. Each example is a library with its
application in `src/lib.rs`, a `src/main.rs` that runs it, and tests under `tests/`. The examples that need a service (a database, a broker) say so, and their service-backed
tests sit behind an `integration` feature that starts the service in a container through Docker.

An example answers one question a user would ask. Add one by creating `examples/NN-name`, adding it
to the workspace `members` in the root `Cargo.toml`, and listing it in the root README.
