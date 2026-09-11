# integration-tests

Where a ulo crate's behaviour is proved when proving it needs more than one crate present.

Nothing here is published. The crate exists so that `ulo` can be tested against an HTTP adapter
without depending on one, and so the adapters, transports and integrations can be tested against
each other rather than against a mock of each other.

## Running it

```bash
cargo test -p integration-tests                      # the whole suite
cargo test -p integration-tests -- ws_session         # one file
cargo test -p integration-tests -- --nocapture        # with handler output
RUST_LOG=ulo=debug cargo test -p integration-tests    # with framework tracing
```

There is one test target, named `integration`: `tests/integration/main.rs` declares every file as a
module, so `--test <filename>` does not address one. Filter by name instead, as above.

Every test binds port 0 and serves on whatever the OS assigns. Nothing here needs a running
service, and nothing may acquire a fixed port.

## What belongs here

A test belongs in this crate when the thing it proves does not exist until two crates are wired
together — dispatch through an adapter, an enhancer reaching a handler, a transport carrying an
error back in the shape the wire promises. That is most of the framework's behaviour, which is why
this suite is large.

A test belongs in a crate's own `tests/` when it is about that library and no other: what actix
does to a body, what the redis broadcast service writes to redis, how a database module reports an
unreachable server. Those suites are also where a service-backed test lives, behind the crate's
`integration` feature.

The dividing question is which crate's change should break the test. A conformance suite here runs
the same six cases against all five HTTP adapters, so a regression in any one of them fails; a test
of actix's 256 KiB payload ceiling belongs to actix, and would be noise in a file the other four
adapters also run.

## Registering a crate

1. Add the path dependency to `Cargo.toml`.
2. Write `tests/integration/<contract>.rs`.
3. Declare it: `mod <contract>;` in `tests/integration/main.rs`. Without this line the file is
   compiled by nothing and run by nothing, and cargo reports neither.
4. Update the crate's line in `coverage_ledger.rs`, which fails until it matches the tree.

## What a file is called, and what it opens with

The file name is the contract, not the subject and not the project phase that produced it:
`guard_rejection_is_an_event.rs`, not `enhancers3.rs`. A name that only a reader who was present
can decode is a name that stops being read.

Every file opens with a `//!` header saying what it proves and, where it is not obvious, what
failure the test exists to catch. `coverage_ledger.rs` enforces the header's presence; the header's
usefulness is on the author. The one to copy is a conformance suite — it states the contract, names
the cases that discriminate, and says which adapter is the exception and why.

## Shared helpers

`tests/integration/common/` holds what more than one file needs:

| Helper | For |
| --- | --- |
| `TestServer` | Boot an application on port 0 and address it. `start_adapter` and `start_target` are the parameterization points for suites that run against every adapter, or hand the application a socket they bound |
| `ExecutionOrder` | Record what ran, in what order, across an enhancer chain |
| `panic_message` | Read the message out of a recovered panic |
| `NotServed` | A gRPC stub for tests that need a service declared but never dialled |

## The ledger

`coverage_ledger.rs` asserts that every crate under `crates/` is proved somewhere, that every
example is reachable from an index, and that this file's claims about registration are true of the
tree. It carries the reasoning.
