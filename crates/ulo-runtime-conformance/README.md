# ulo-runtime-conformance

The conformance suite every `ulo` runtime runs. A runtime is what `ulo` spawns tasks and sleeps
through: `ulo-runtime-tokio`, `ulo-runtime-smol`, or one written outside the `ulo` repository. One
macro stamps the suite's scenarios as `#[test]`s against the runtime: what a task's handle answers
after a return, an abort, a panic and a drop, what `spawn_with` and a `TaskSet` answer over it,
blocking work running on threads that do not poll the runtime's tasks, an app binding the runtime as
one object that redacts a spawned task's panic with the app's secrets, and a runtime that has shut
down refusing to spawn, to run blocking work, to time and to run a future, without a panic. A
runtime whose `tests/conformance.rs` passes keeps the same promises as the two `ulo` ships, each of
which runs the suite in CI.

## Install

The suite is a dev-dependency of the runtime crate. The example below runs it against
`ulo-runtime-smol`, which stands in for your runtime:

```toml
[dev-dependencies]
ulo-runtime-conformance = "=0.1.0-alpha.3"
ulo-runtime-smol = "=0.1.0-alpha.3"
```

Each crate is pinned with `=`, since a caret on a pre-release also takes a later alpha, which may
break this one.

## First run

`tests/conformance.rs`:

```rust
use std::future::Future;
use std::sync::Arc;

use ulo_runtime_conformance::{Harness, Stoppable, runtime_suite};
use ulo_runtime_smol::{Executor, Smol};

struct OnSmol;

impl Harness for OnSmol {
    type Runtime = Smol;

    /// Called inside `block_on`, on a thread running the scenario's executor.
    fn runtime() -> Smol {
        Smol::current().expect("`runtime()` is called inside `block_on`")
    }

    /// An executor of its own per scenario, run by the test's thread and three more.
    fn block_on<F: Future>(fut: F) -> F::Output {
        Smol::new(Arc::new(Executor::new())).with_workers(3).block_on(fut)
    }

    /// The threads running that executor: the test's and the three.
    fn workers() -> usize {
        4
    }

    fn stoppable() -> Option<Stoppable<Smol>> {
        None
    }
}

runtime_suite!(OnSmol; not_applicable {
    a_shut_down_runtime_refuses_spawning_and_timing_without_panicking:
        "smol has no runtime value that can be shut down while code still holds it",
    run_on_a_shut_down_runtime_answers_runtime_shut_down:
        "smol has no runtime value that can be shut down while code still holds it",
});
```

`cargo test --test conformance` runs every scenario as a test of its own, named after the scenario,
and reports the two declared above as ignored with their reasons. A failing scenario reads as an
ordinary failed test, its panic message saying what the runtime did. With the `not_applicable`
block left out, the two shut-down scenarios fail:

```text
test a_shut_down_runtime_refuses_spawning_and_timing_without_panicking ... FAILED

---- a_shut_down_runtime_refuses_spawning_and_timing_without_panicking stdout ----
thread 'a_shut_down_runtime_refuses_spawning_and_timing_without_panicking' panicked at tests/conformance.rs:32:1:
the harness answers no runtime that can shut down: declare `a_shut_down_runtime_refuses_spawning_and_timing_without_panicking` not applicable with the reason
```

Your runtime takes `Smol`'s place. `runtime` answers a value of it, called inside the future
`block_on` runs. `block_on` runs a future to its end from a plain test thread, on an executor that
runs the tasks the runtime spawns meanwhile. `workers` answers how many threads poll those tasks.
`stoppable` answers a second runtime, running, with what shuts it down, for the scenarios on a
runtime that has shut down: a `Stoppable` carries the runtime, the crate's other clocks on it,
`run`, the runtime crate's own way to run a future as a task, and `stop`. [`ulo-runtime-tokio`'s harness](https://github.com/ulo-rs/ulo/blob/master/crates/ulo-runtime-tokio/tests/conformance.rs)
fills all five.

## What a runtime declares

A runtime that cannot shut down while a value of it is held, as `Smol` cannot, answers `None` from
`stoppable` and declares the two shut-down scenarios in `not_applicable { scenario: "reason" }`.
Each is stamped `#[ignore = "<reason>"]`, a name that is no scenario fails to compile, and
`cargo test -- --ignored` runs it, where it fails: a harness answering `None` without the
declaration fails the scenario, naming the declaration to write.

## Scenarios

The scenarios are the functions of the [`cases`](https://docs.rs/ulo-runtime-conformance/latest/ulo_runtime_conformance/cases/)
modules, each taking the runtime by value, so a runtime crate runs one alone to debug it.

## Will it work for me

- **Rust 1.88** or later.
- **Runtimes:** the suite names none and depends on none. Every wait a scenario makes is bounded
  through the runtime's own `Timer`, so a runtime that loses a task fails the scenario after
  `cases::PATIENCE`, five seconds, instead of hanging it.
- **Blocking threads:** one scenario holds `cases::blocking::held_at_once(workers)` pieces of
  blocking work at once, one more than the harness's `workers` and at least four, each waiting for
  a release a task of the runtime gives. The property is that blocking work does not stall the
  runtime's tasks, and that needs the workers outnumbered, not a large pool: a runtime running
  blocking work on the threads that poll its tasks has every one of them held and fails, and one
  whose blocking pool runs that many pieces at once passes.
- **Output:** a scenario that observes a panic causes one inside a task, and the panic hook prints
  it to standard error while the scenario passes.
- **Logging:** the two scenarios reading the core's `warn` install a recording `tracing`
  subscriber as the global default, once per test binary, so a harness installs none of its own.
  In a binary that installed one first they fail with `cases::SUBSCRIBER_TAKEN`.
- **Stability:** the scenarios, `Harness`, `Stoppable` and `runtime_suite!` are the contract a
  runtime is held to. While the version is a pre-release, every `ulo` crate releases at one version
  and a pre-release may change any of them. From `0.1.0` on, a release that adds or tightens a
  scenario or changes the contract is a minor release, `0.1` to `0.2`, since a runtime that passed
  the earlier suite can fail it.

## Where to go next

- The API docs on [docs.rs](https://docs.rs/ulo-runtime-conformance), or `cargo doc --open`.
- The harnesses of the runtimes `ulo` ships:
  [`ulo-runtime-tokio`](https://github.com/ulo-rs/ulo/blob/master/crates/ulo-runtime-tokio/tests/conformance.rs)
  and [`ulo-runtime-smol`](https://github.com/ulo-rs/ulo/blob/master/crates/ulo-runtime-smol/tests/conformance.rs),
  run on every build.

## License

MIT.
