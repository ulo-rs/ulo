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

#[test]
#[should_panic(expected = "self-test until: the condition did not happen within")]
fn a_bounded_wait_that_expires_fails_naming_the_loop() {
    Starvation::new("self-test until").run(|probe| async move {
        probe.until("the condition", Duration::from_millis(50), || false).await;
    });
}
