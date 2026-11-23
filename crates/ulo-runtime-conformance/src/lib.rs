#![doc = include_str!("../README.md")]

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

    /// How many threads poll the tasks of the runtime [`runtime`](Self::runtime) answers, the
    /// thread running `block_on` among them where it polls tasks too. The blocking scenario hands
    /// over more pieces of blocking work than this, so a runtime running blocking work on those
    /// threads has none left for the task that releases them. Called inside the future `block_on`
    /// runs.
    fn workers() -> usize;

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
/// `#[ignore = "<reason>"]`, so the test report counts it as ignored rather than passed and prints
/// the reason beside it. A harness whose `stoppable` answers `None` declares both shut-down
/// scenarios; the crate's README shows the whole harness:
///
/// ```ignore
/// # // reason: names a harness type defined in the runtime crate's own tests/conformance.rs
/// ulo_runtime_conformance::runtime_suite!(OnSmol; not_applicable {
///     a_shut_down_runtime_refuses_spawning_and_timing_without_panicking: "a `Smol` cannot shut down",
///     run_on_a_shut_down_runtime_answers_runtime_shut_down: "a `Smol` cannot shut down",
/// });
/// ```
///
/// A declared name that is no scenario fails to compile, and so does a name declared twice:
///
/// ```compile_fail
/// # use std::future::Future;
/// # use std::sync::Arc;
/// # use ulo_runtime_conformance::{Harness, Stoppable, runtime_suite};
/// # use ulo_runtime_smol::{Executor, Smol};
/// # struct OnSmol;
/// # impl Harness for OnSmol {
/// #     type Runtime = Smol;
/// #     fn runtime() -> Smol { Smol::current().expect("inside `block_on`") }
/// #     fn block_on<F: Future>(fut: F) -> F::Output { Smol::new(Arc::new(Executor::new())).block_on(fut) }
/// #     fn workers() -> usize { 1 }
/// #     fn stoppable() -> Option<Stoppable<Smol>> { None }
/// # }
/// runtime_suite!(OnSmol; not_applicable {
///     a_shut_down_runtime_refuses_spawning_and_timing_without_panicking: "a `Smol` cannot shut down",
///     run_on_a_shut_down_runtime_answers_runtime_shut_down: "a `Smol` cannot shut down",
///     run_on_a_shut_down_runtime_answers_runtime_shut_down: "a second reason for the same scenario",
/// });
/// ```
///
/// The shut-down scenario run on a harness whose `stoppable` answers `None` fails rather than
/// passing, so a runtime leaving out the declaration fails too.
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
                    // A literal: libtest prints an `ignore` reason only when it is one, and drops
                    // a `concat!`'s.
                    #[ignore = $why]
                    $d($d test)*
                };
            )*
            ($d other:ident, $d($d test:tt)*) => {
                #[test]
                $d($d test)*
            };
        }
        // A declared name that is no scenario names no constant in `__ulo_runtime_conformance_scenarios`,
        // a test build or any other: the stamped functions exist in a test build alone, and a
        // doctest is none.
        $(const _: () = __ulo_runtime_conformance_scenarios::$skip;)*
        // A name declared twice is defined twice here (E0428). The stamper would match the first
        // declaration alone and drop the second's reason.
        #[allow(dead_code, non_upper_case_globals)]
        mod __ulo_runtime_conformance_declared {
            $(pub(super) const $skip: () = ();)*
        }
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
            blocking_work_runs_beside_the_runtime_s_tasks => blocking::beside_tasks [workers],
            blocking_work_runs_off_the_caller_s_thread_and_answers_its_value => blocking::answered,
            blocking_work_that_panics_is_panicked_and_goes_no_further => blocking::panicked,
            the_app_binds_the_runtime_as_one_object => app::one_object,
            a_task_spawned_through_the_app_redacts_its_secrets => app::redacted,
            a_running_runtime_refuses_neither_spawning_nor_timing => refusal::running,
            a_shut_down_runtime_refuses_spawning_and_timing_without_panicking => refusal::shut_down [stoppable],
            run_on_a_shut_down_runtime_answers_runtime_shut_down => refusal::run_refused [stoppable],
        );
    };
    (@cases $harness:ty; $($name:ident => $module:ident :: $case:ident $([$takes:ident])?),* $(,)?) => {
        #[allow(dead_code, non_upper_case_globals)]
        mod __ulo_runtime_conformance_scenarios {
            $(pub(super) const $name: () = ();)*
        }
        $(
            __ulo_runtime_conformance_stamp!($name, fn $name() {
                <$harness as $crate::Harness>::block_on(async {
                    $crate::runtime_suite!(@run $harness, $name, $module::$case $(, $takes)?).await
                });
            });
        )*
    };
    // A scenario on a runtime that has shut down takes what the harness's `stoppable` answers, and
    // fails rather than passes on `None`.
    (@run $harness:ty, $name:ident, $module:ident :: $case:ident, stoppable) => {
        $crate::cases::$module::$case(
            <$harness as $crate::Harness>::runtime(),
            <$harness as $crate::Harness>::stoppable().expect(concat!(
                "the harness answers no runtime that can shut down: declare `",
                stringify!($name),
                "` not applicable with the reason",
            )),
        )
    };
    (@run $harness:ty, $name:ident, $module:ident :: $case:ident, workers) => {
        $crate::cases::$module::$case(<$harness as $crate::Harness>::runtime(), <$harness as $crate::Harness>::workers())
    };
    (@run $harness:ty, $name:ident, $module:ident :: $case:ident) => {
        $crate::cases::$module::$case(<$harness as $crate::Harness>::runtime())
    };
}
