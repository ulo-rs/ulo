//! The conformance suite every `ulo` runtime runs: what a [`TaskHandle`](ulo::TaskHandle) answers
//! after a return, an abort, a panic and a drop, what [`spawn_with`](ulo::spawn_with) and a
//! [`TaskSet`](ulo_transport::TaskSet) answer over it, and the app binding the runtime as one
//! object that redacts a spawned task's panic with the app's secrets, and a runtime that has shut
//! down refusing to spawn, to time and to run a future without a panic. One scenario list, stamped
//! per runtime by [`runtime_suite!`] from a [`Harness`] the runtime crate implements in its
//! `tests/`.
//!
//! ```ignore
//! # // reason: a template for a runtime crate's tests/conformance.rs: it elides the harness's bodies
//! // crates/ulo-runtime-tokio/tests/conformance.rs
//! struct OnTokio;
//!
//! impl ulo_runtime_conformance::Harness for OnTokio {
//!     type Runtime = ulo_runtime_tokio::Tokio;
//!     fn runtime() -> ulo_runtime_tokio::Tokio { ulo_runtime_tokio::Tokio::current() }
//!     fn block_on<F: Future>(fut: F) -> F::Output { /* a runtime of its own per test */ }
//!     fn stoppable() -> Option<Stoppable<ulo_runtime_tokio::Tokio>> { /* another, and its shutdown */ }
//! }
//!
//! ulo_runtime_conformance::runtime_suite!(OnTokio);
//! ```
//!
//! The suite names no runtime and depends on none. Every wait a scenario makes is bounded through
//! the runtime's own `Timer`, so a runtime that loses a task fails the scenario after
//! [`cases::PATIENCE`] instead of hanging it. A scenario that observes a panic causes one inside a
//! task, and the panic hook prints it to stderr while the scenario passes.
//!
//! The two scenarios reading the core's `warn` install a recording `tracing` subscriber as the
//! global default, once per test binary, so a runtime crate's harness installs none of its own. In
//! a binary that installed one first they fail with [`cases::SUBSCRIBER_TAKEN`].

use std::future::Future;

use ulo::{BoxError, BoxFuture, Runtime, Timer};

pub mod cases;

/// One runtime as the suite drives it.
pub trait Harness {
    type Runtime: Runtime;

    /// A runtime value; each scenario takes its own, built inside the future `block_on` runs, so
    /// a runtime that captures the executor it runs on finds it.
    fn runtime() -> Self::Runtime;

    /// Runs `fut` to its end from a plain `#[test]` thread, on an executor that runs the tasks
    /// the runtime spawns while `fut` waits. Each scenario calls it once.
    fn block_on<F: Future>(fut: F) -> F::Output;

    /// A runtime of its own, running, with what shuts it down, for the scenarios on a runtime that
    /// has shut down. `None` for a runtime that cannot shut down while a value of it is held,
    /// whose suite then declares those scenarios not applicable. Called inside the future
    /// `block_on` runs.
    fn stoppable() -> Option<Stoppable<Self::Runtime>>;
}

/// A running runtime and what shuts it down, as [`Harness::stoppable`] answers them.
pub struct Stoppable<R> {
    pub runtime: R,
    /// The runtime crate's other clocks on that same runtime, each by the name a failure reports
    /// it under, held to `runtime`'s rule: they time while it runs and refuse once it has shut
    /// down. Never empty, so a harness that leaves its clocks out fails the scenario rather than
    /// passing it on none: a runtime crate whose runtime is its only clock lists a second value of
    /// the runtime.
    pub clocks: Vec<(&'static str, Box<dyn Timer>)>,
    /// Runs a future as a task on that runtime and answers its output, as `Tokio::run` does; the
    /// scenario requires a [`RuntimeShutDown`](ulo::RuntimeShutDown) once the runtime has shut down.
    pub run: Run,
    /// Returns once the runtime has shut down. Called from inside the future `block_on` runs.
    pub stop: Box<dyn FnOnce() + Send>,
}

/// [`Stoppable::run`]: the runtime crate's own way to run a future on its runtime, its refusal boxed.
pub type Run = Box<dyn Fn(BoxFuture<'static, ()>) -> BoxFuture<'static, Result<(), BoxError>> + Send>;

/// Stamps one `#[test]` per scenario in [`cases`] for the [`Harness`] given.
///
/// A scenario that cannot apply to a runtime is declared with its reason, and stamped
/// `#[ignore = "not applicable: <reason>"]`, so the test report counts it as ignored rather than
/// passed:
///
/// ```ignore
/// # // reason: names a harness type defined in the runtime crate's own tests/conformance.rs
/// ulo_runtime_conformance::runtime_suite!(OnSmol; not_applicable {
///     a_shut_down_runtime_refuses_spawning_and_timing_without_panicking: "a `Smol` cannot shut down",
/// });
/// ```
///
/// A declared name that is no scenario fails to compile. The shut-down scenario run on a harness
/// whose `stoppable` answers `None` fails rather than passing, so a runtime leaving out the
/// declaration fails too.
#[macro_export]
macro_rules! runtime_suite {
    ($harness:ty $(,)?) => {
        $crate::runtime_suite!($harness; not_applicable {});
    };
    ($harness:ty; not_applicable { $($skip:ident : $why:literal),* $(,)? } $(,)?) => {
        $crate::runtime_suite!(@stamper [$] $harness; $($skip : $why),*);
    };
    (@stamper [$d:tt] $harness:ty; $($skip:ident : $why:literal),*) => {
        macro_rules! __ulo_runtime_conformance_stamp {
            $(
                ($skip, $d($d test:tt)*) => {
                    #[test]
                    #[ignore = concat!("not applicable: ", $why)]
                    $d($d test)*
                };
            )*
            ($d other:ident, $d($d test:tt)*) => {
                #[test]
                $d($d test)*
            };
        }
        // A declared name that is no scenario names no function here.
        $(const _: fn() = $skip;)*
        $crate::runtime_suite!(@cases $harness;
            a_returned_future_is_finished => task::finished,
            a_dropped_handle_detaches => task::detached,
            abort_is_aborted_once_the_future_is_dropped => task::aborted,
            a_panic_is_panicked_and_goes_no_further => task::panicked,
            an_end_is_kept_and_abort_after_it_changes_nothing => task::end_kept,
            a_panic_while_an_aborted_future_drops_is_aborted_and_logged => task::panic_while_aborted,
            spawn_with_answers_the_value => value::answered,
            spawn_with_answers_finished_once_the_value_is_taken => value::taken,
            spawn_with_answers_an_abort => value::aborted,
            spawn_with_answers_a_panic => value::panicked,
            abort_all_aborts_every_task => set::abort_all,
            join_all_returns_once_every_task_has_ended => set::join_all,
            a_dropped_set_aborts_its_tasks => set::dropped,
            the_app_binds_the_runtime_as_one_object => app::one_object,
            a_task_spawned_through_the_app_redacts_its_secrets => app::redacted,
            a_running_runtime_refuses_neither_spawning_nor_timing => refusal::running,
        );

        $crate::runtime_suite!(@stoppable $harness;
            a_shut_down_runtime_refuses_spawning_and_timing_without_panicking => refusal::shut_down,
            run_on_a_shut_down_runtime_answers_runtime_shut_down => refusal::run_refused,
        );
    };
    (@stoppable $harness:ty; $($name:ident => $module:ident :: $case:ident),* $(,)?) => {
        $(
            __ulo_runtime_conformance_stamp!($name, fn $name() {
                <$harness as $crate::Harness>::block_on(async {
                    let stoppable = <$harness as $crate::Harness>::stoppable().expect(concat!(
                        "the harness answers no runtime that can shut down: declare `",
                        stringify!($name),
                        "` not applicable with the reason",
                    ));
                    $crate::cases::$module::$case(<$harness as $crate::Harness>::runtime(), stoppable).await
                });
            });
        )*
    };
    (@cases $harness:ty; $($name:ident => $module:ident :: $case:ident),* $(,)?) => {
        $(
            __ulo_runtime_conformance_stamp!($name, fn $name() {
                <$harness as $crate::Harness>::block_on(async {
                    $crate::cases::$module::$case(<$harness as $crate::Harness>::runtime()).await
                });
            });
        )*
    };
}
