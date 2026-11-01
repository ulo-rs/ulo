//! The test shape every starvation case shares (transports DESIGN §3.7): a loop that reads a peer
//! which never stops writing must hand the executor a turn now and then, or every other task on
//! its thread waits for it. A case names the loop it pins and runs on a one-thread smol executor
//! beside a ticker task, against a flooding peer on threads of its own, under a watchdog the
//! executor cannot starve.
//!
//! The ticker is a task that polls, counts and wakes itself, so it runs once per turn the
//! executor offers it. [`Probe::meter`] is a count the code under test advances, a socket
//! wrapper adding the bytes it reads; the ticker samples it on every turn and keeps the largest
//! advance between two samples, [`Probe::max_gap`]. A loop that yields every N reads bounds the
//! gap at N reads whatever the machine's load, which a duration cannot.
//!
//! The watchdog runs on the test's own thread. It fails the case, naming the loop, when the
//! ticker runs no turn for four [`Budget::settle`], and when the scenario has not ended within
//! [`Budget::boot`] plus [`Budget::recovery`] plus the time the case [`allow`](Starvation::allow)s.
//! Every wait a scenario makes goes through [`Probe::until`], which fails the same way, so no
//! starvation case ends in a kill from the outside.

use std::future::{Future, poll_fn};
use std::io;
use std::net::{Shutdown, TcpStream};
use std::panic::resume_unwind;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::task::Poll;
use std::thread;
use std::time::{Duration, Instant};

use ulo::{Spawn, Timer};
use ulo_runtime_smol::{Executor, Smol};

use crate::Budget;

/// How often the watchdog and [`Probe::until`] look.
const LOOK: Duration = Duration::from_millis(2);

/// One starvation case: the loop it pins and the bounds its waits are scaled from.
#[derive(Clone, Debug)]
pub struct Starvation {
    loop_name: &'static str,
    budget: Budget,
    allowed: Duration,
}

impl Starvation {
    /// A case pinning `loop_name`, which every failure names: the loop whose yield the case
    /// removes the need for, spelled as a reader of the code finds it.
    pub fn new(loop_name: &'static str) -> Self {
        Starvation { loop_name, budget: Budget::default(), allowed: Duration::ZERO }
    }

    pub fn budget(self, budget: Budget) -> Self {
        Starvation { budget, ..self }
    }

    /// Time the scenario spends waiting by design, a drain it closes an app through, added to the
    /// deadline of the whole scenario.
    pub fn allow(self, extra: Duration) -> Self {
        Starvation { allowed: extra, ..self }
    }

    /// Runs `scenario` on a one-thread executor beside the ticker, to its end. Panics, naming the
    /// loop, when the watchdog trips; a panic of the scenario itself is resumed as it was.
    pub fn run<F, Fut>(self, scenario: F)
    where
        F: FnOnce(Probe) -> Fut + Send + 'static,
        Fut: Future<Output = ()>,
    {
        let shared = Arc::new(Shared::default());
        let probe_shared = Arc::clone(&shared);
        let (loop_name, budget) = (self.loop_name, self.budget);
        let (ended, end) = mpsc::channel::<()>();
        let executor = thread::Builder::new()
            .name(format!("starvation: {loop_name}"))
            .spawn(move || {
                let smol = Smol::new(Arc::new(Executor::new()));
                smol.block_on(async move {
                    let runtime = Smol::current().expect("`block_on` runs the scenario on the executor's thread");
                    let probe = Probe { loop_name, budget, shared: Arc::clone(&probe_shared), runtime: runtime.clone() };
                    drop(runtime.spawn(Box::pin(tick(Arc::clone(&probe_shared)))));
                    scenario(probe).await;
                    probe_shared.stop.store(true, Ordering::Release);
                });
                let _ = ended.send(());
            })
            .expect("the executor's thread starts");

        let stall = budget.settle * 4;
        let deadline = budget.boot + budget.recovery + self.allowed;
        let began = Instant::now();
        let (mut ticks, mut moved) = (0, began);
        loop {
            match end.recv_timeout(LOOK) {
                Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            let now = shared.ticks.load(Ordering::Acquire);
            if now != ticks {
                (ticks, moved) = (now, Instant::now());
            } else if moved.elapsed() > stall {
                panic!(
                    "{loop_name} starved the executor: the ticker on its thread ran no turn for {stall:?} ({ticks} ticks in {:?}); the loop does not yield to the other tasks on its thread",
                    began.elapsed()
                );
            }
            if began.elapsed() > deadline {
                panic!(
                    "{loop_name}: the scenario did not end within {deadline:?}, though the ticker kept running ({ticks} ticks): a wakeup was lost, or the loop never finished its work"
                );
            }
        }
        if let Err(payload) = executor.join() {
            resume_unwind(payload);
        }
    }
}

#[derive(Default)]
struct Shared {
    ticks: AtomicU64,
    meter: AtomicU64,
    max_gap: AtomicU64,
    stop: AtomicBool,
}

/// The ticker: one turn per time the executor offers it, so a loop that never yields leaves it
/// unpolled.
async fn tick(shared: Arc<Shared>) {
    let mut sampled = 0;
    while !shared.stop.load(Ordering::Acquire) {
        let metered = shared.meter.load(Ordering::Acquire);
        shared.max_gap.fetch_max(metered - sampled, Ordering::AcqRel);
        sampled = metered;
        shared.ticks.fetch_add(1, Ordering::AcqRel);
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
}

/// What a scenario holds: the runtime it runs on, the ticker's counts and the bounded wait.
#[derive(Clone)]
pub struct Probe {
    loop_name: &'static str,
    budget: Budget,
    shared: Arc<Shared>,
    runtime: Smol,
}

/// A count the code under test advances and the ticker samples.
#[derive(Clone)]
pub struct Meter(Arc<Shared>);

impl Meter {
    pub fn add(&self, by: u64) {
        self.0.meter.fetch_add(by, Ordering::AcqRel);
    }
}

impl Probe {
    /// The app's runtime, on the case's executor.
    pub fn runtime(&self) -> Smol {
        self.runtime.clone()
    }

    pub fn budget(&self) -> Budget {
        self.budget
    }

    /// The turns the ticker has run.
    pub fn ticks(&self) -> u64 {
        self.shared.ticks.load(Ordering::Acquire)
    }

    pub fn meter(&self) -> Meter {
        Meter(Arc::clone(&self.shared))
    }

    /// The largest advance of the meter between two turns of the ticker since the last
    /// [`reset_gap`](Self::reset_gap).
    pub fn max_gap(&self) -> u64 {
        self.shared.max_gap.load(Ordering::Acquire)
    }

    /// The meter's count so far.
    pub fn metered(&self) -> u64 {
        self.shared.meter.load(Ordering::Acquire)
    }

    /// Fails, naming the loop, unless the meter advanced by at most `limit` between any two turns
    /// of the ticker since the last [`reset_gap`](Self::reset_gap): the loop gave the executor a
    /// turn at least that often.
    pub fn expect_gap_within(&self, limit: u64, unit: &str) {
        let gap = self.max_gap();
        assert!(
            gap <= limit,
            "{} starved the executor: it advanced {gap} {unit} between two turns of the ticker, over the {limit} it may",
            self.loop_name
        );
    }

    pub fn reset_gap(&self) {
        self.shared.max_gap.store(0, Ordering::Release);
    }

    /// Waits until `ready` holds, for at most `bound`; on expiry fails naming the loop and `what`.
    pub async fn until(&self, what: &str, bound: Duration, mut ready: impl FnMut() -> bool) {
        let began = Instant::now();
        while !ready() {
            if began.elapsed() > bound {
                panic!("{}: {what} did not happen within {bound:?}", self.loop_name);
            }
            self.runtime.sleep(LOOK).await;
        }
    }

    /// [`until`](Self::until) for a one-way effect to show, bounded by [`Budget::settle`].
    pub async fn settles(&self, what: &str, ready: impl FnMut() -> bool) {
        self.until(what, self.budget.settle, ready).await;
    }

    /// Waits until the ticker has run `turns` more turns than it has now.
    pub async fn ticks_advance(&self, turns: u64) {
        let target = self.ticks() + turns;
        self.settles(&format!("the ticker running {turns} more turns"), || self.ticks() >= target).await;
    }

    /// Runs `fut` and, when it does not end within `bound`, fails naming the loop and `what`.
    /// `fut` is polled again only when it is woken or the bound passes, so a lost wakeup in it
    /// shows as the failure rather than being papered over.
    pub async fn within<T>(&self, what: &str, bound: Duration, fut: impl Future<Output = T>) -> T {
        let mut fut = std::pin::pin!(fut);
        let mut expired = self.runtime.sleep(bound);
        poll_fn(|cx| {
            if let Poll::Ready(output) = fut.as_mut().poll(cx) {
                return Poll::Ready(output);
            }
            if expired.as_mut().poll(cx).is_ready() {
                panic!("{}: {what} did not end within {bound:?}", self.loop_name);
            }
            Poll::Pending
        })
        .await
    }
}

/// A peer writing to a socket from a thread of its own, started by [`Flood::repeat`] or
/// [`Flood::once`]. Dropping it stops the thread, which a write blocked on a peer that reads
/// nothing does by the socket being shut.
pub struct Flood {
    stop: Arc<AtomicBool>,
    written: Arc<AtomicU64>,
    done: Arc<AtomicBool>,
    failure: Arc<Mutex<Option<io::Error>>>,
    socket: TcpStream,
}

impl Flood {
    /// Writes `batch` over and over until stopped or a write fails.
    pub fn repeat(socket: &TcpStream, batch: Vec<u8>) -> Flood {
        Flood::start(socket, batch, true)
    }

    /// Writes `bytes` once, with one `write_all`, then holds the socket open and silent.
    pub fn once(socket: &TcpStream, bytes: Vec<u8>) -> Flood {
        Flood::start(socket, bytes, false)
    }

    fn start(socket: &TcpStream, batch: Vec<u8>, repeat: bool) -> Flood {
        let flood = Flood {
            stop: Arc::default(),
            written: Arc::default(),
            done: Arc::default(),
            failure: Arc::default(),
            socket: socket.try_clone().expect("the peer's socket clones"),
        };
        let (stop, written, done, failure) = (Arc::clone(&flood.stop), Arc::clone(&flood.written), Arc::clone(&flood.done), Arc::clone(&flood.failure));
        let mut writing = socket.try_clone().expect("the peer's socket clones");
        thread::spawn(move || {
            use std::io::Write;
            while !stop.load(Ordering::Acquire) {
                if let Err(error) = writing.write_all(&batch) {
                    *failure.lock().unwrap_or_else(PoisonError::into_inner) = Some(error);
                    break;
                }
                written.fetch_add(batch.len() as u64, Ordering::Release);
                if !repeat {
                    break;
                }
            }
            done.store(true, Ordering::Release);
        });
        flood
    }

    /// The bytes written so far.
    pub fn written(&self) -> u64 {
        self.written.load(Ordering::Acquire)
    }

    /// Whether the writing thread has ended, on a failure, on the single write, or stopped.
    pub fn ended(&self) -> bool {
        self.done.load(Ordering::Acquire)
    }

    /// The write that failed, once the thread has ended on one.
    pub fn failure(&self) -> Option<io::Error> {
        self.failure.lock().unwrap_or_else(PoisonError::into_inner).take()
    }

    /// Asks the thread to stop after its current write.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Release);
    }
}

impl Drop for Flood {
    fn drop(&mut self) {
        self.stop();
        let _ = self.socket.shutdown(Shutdown::Both);
    }
}
