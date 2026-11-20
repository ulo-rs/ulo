//! The starvation harness against loops whose behaviour is known: each way a case can fail is
//! shown to fail, naming the loop, and a loop that yields often enough is shown to pass.

use std::future::poll_fn;
use std::task::Poll;
use std::time::{Duration, Instant};

use ulo::Timer;
use ulo_conformance_support::Budget;
use ulo_conformance_support::starvation::Starvation;

/// A stall bound of 200 ms and a scenario deadline of 200 ms, so each failing case fails fast.
const SHORT: Budget = Budget::new(Duration::from_millis(100), Duration::from_millis(50), Duration::from_millis(100));

/// Steps each loop takes, the meter advancing by one a step.
const STEPS: u64 = 10_000;

async fn yield_now() {
    let mut yielded = false;
    poll_fn(|cx| {
        if yielded {
            return Poll::Ready(());
        }
        yielded = true;
        cx.waker().wake_by_ref();
        Poll::Pending
    })
    .await;
}

#[test]
fn a_loop_yielding_every_eight_steps_stays_within_eight() {
    Starvation::new("self-test loop yielding every 8 steps").run(|probe| async move {
        let meter = probe.meter();
        probe.ticks_advance(1).await;
        probe.reset_gap();
        for step in 1..=STEPS {
            meter.add(1);
            if step % 8 == 0 {
                yield_now().await;
            }
        }
        probe.expect_gap_within(8, "steps");
    });
}

#[test]
#[should_panic(expected = "self-test loop never yielding starved the executor: it advanced 10000 steps")]
fn a_loop_that_never_yields_fails_the_gap() {
    Starvation::new("self-test loop never yielding").run(|probe| async move {
        let meter = probe.meter();
        probe.ticks_advance(1).await;
        probe.reset_gap();
        for _ in 0..STEPS {
            meter.add(1);
        }
        probe.ticks_advance(1).await;
        probe.expect_gap_within(8, "steps");
    });
}

/// The scenario holds the thread for a second, past the 200 ms stall bound, then ends.
#[test]
#[should_panic(expected = "self-test spin starved the executor: the ticker on its thread ran no turn")]
fn a_scenario_holding_its_thread_trips_the_watchdog() {
    Starvation::new("self-test spin").budget(SHORT).run(|_probe| async move {
        let began = Instant::now();
        while began.elapsed() < Duration::from_secs(1) {
            std::hint::spin_loop();
        }
    });
}

/// The scenario waits a second, past the 200 ms deadline, with the ticker running.
#[test]
#[should_panic(expected = "self-test wait: the scenario did not end within")]
fn a_scenario_past_its_deadline_fails_naming_the_loop() {
    Starvation::new("self-test wait").budget(SHORT).run(|probe| async move {
        probe.runtime().sleep(Duration::from_secs(1)).await;
    });
}

/// How many steps the progress below takes, one per look of the waiting loop: at least 2 ms a
/// look, so 100 ms or more in all, past [`BOUND`] however idle the machine.
const PROGRESS: u64 = 50;

/// The bound both waits below are given.
const BOUND: Duration = Duration::from_millis(50);

/// A wait bounded by its total fails a flood that keeps progressing but takes longer than the
/// bound, which is what a loaded machine makes of one (F671): the progress here is one step per
/// look, so the total is past the bound on any machine.
#[test]
#[should_panic(expected = "self-test progress: the progress did not happen within 50ms")]
fn a_wait_bounded_by_its_total_fails_progress_that_never_stops() {
    Starvation::new("self-test progress").run(|probe| async move {
        let mut read = 0;
        probe
            .until("the progress", BOUND, || {
                read += 1;
                read >= PROGRESS
            })
            .await;
    });
}

/// The same progress under a bound on progress reaches its target: it never stays still for a
/// look. Planted break: `until_reaches` counting every look rather than the still ones.
#[test]
fn a_wait_bounded_by_progress_passes_progress_that_never_stops() {
    Starvation::new("self-test progress").run(|probe| async move {
        let mut read = 0;
        probe
            .until_reaches(
                "the progress",
                BOUND,
                || {
                    read += 1;
                    read
                },
                PROGRESS,
            )
            .await;
    });
}

/// Progress that pauses, again and again, for fewer looks than the bound reaches its target: the
/// bound counts the looks since the last step, not since the wait began. Planted break: the
/// count of still looks not reset by a step.
#[test]
fn a_wait_bounded_by_progress_passes_progress_that_pauses_short_of_the_bound() {
    Starvation::new("self-test progress").run(|probe| async move {
        let mut looks = 0;
        probe
            .until_reaches(
                "the progress",
                BOUND,
                || {
                    looks += 1;
                    looks / 5
                },
                PROGRESS,
            )
            .await;
    });
}

#[test]
#[should_panic(expected = "self-test progress: the progress stopped at 7 of 50 for 50ms")]
fn a_wait_bounded_by_progress_fails_progress_that_stops() {
    Starvation::new("self-test progress").run(|probe| async move {
        let mut read = 0;
        probe
            .until_reaches(
                "the progress",
                BOUND,
                || {
                    read = (read + 1).min(7);
                    read
                },
                PROGRESS,
            )
            .await;
    });
}

#[test]
#[should_panic(expected = "self-test until: the condition did not happen within")]
fn a_bounded_wait_that_expires_fails_naming_the_loop() {
    Starvation::new("self-test until").run(|probe| async move {
        probe.until("the condition", Duration::from_millis(50), || false).await;
    });
}
