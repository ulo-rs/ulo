//! Load shedding (transports DESIGN §2.8): a runtime-free semaphore with two counters, one per
//! server and one per connection where a connection exists. Over a limit a call is refused at
//! once, never queued; each transport refuses in its protocol's own way, and the refusals are
//! counted into a [`ShedLog`], a `warn` written at most once per [`SHED_LOG_INTERVAL`].

use std::panic::AssertUnwindSafe;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use async_lock::{Semaphore, SemaphoreGuardArc};
use ulo::Timer;

/// A server's in-flight bound. Clone it into each connection task; every clone counts against the
/// same limit.
#[derive(Clone)]
pub struct Admission {
    server: Option<Arc<Semaphore>>,
    retry_after: Duration,
    log: Option<ShedLog>,
}

/// One connection's in-flight bound, nested in its server's.
#[derive(Clone)]
pub struct ConnectionAdmission {
    server: Admission,
    connection: Option<Arc<Semaphore>>,
}

/// One admitted call. Dropping it frees its place in both counters.
pub struct Permit {
    _server: Option<SemaphoreGuardArc>,
    _connection: Option<SemaphoreGuardArc>,
}

impl Admission {
    /// The server's bound: `None` admits every call. A refusal's `Retry-After` is one second
    /// until [`retry_after`](Self::retry_after) sets it, from the server's `.shed_retry_after(..)`.
    /// A refusal is counted nowhere until [`logged`](Self::logged) names a [`ShedLog`].
    pub fn new(limit: Option<usize>) -> Self {
        Admission { server: limit.map(|limit| Arc::new(Semaphore::new(limit))), retry_after: Duration::from_secs(1), log: None }
    }

    pub fn retry_after(self, after: Duration) -> Self {
        Admission { retry_after: after, ..self }
    }

    /// Counts every refusal of this server's, and of each of its connections', into `log`. Every
    /// transport the framework ships names one built with its own key and the app's `Timer`; a
    /// transport written outside it calls this to be seen shedding the same way.
    pub fn logged(self, log: ShedLog) -> Self {
        Admission { log: Some(log), ..self }
    }

    /// What a refusal carries as `Retry-After`, where its protocol has one.
    pub fn shed_retry_after(&self) -> Duration {
        self.retry_after
    }

    /// A place for one call, or `None` over the server's limit, the refusal counted into the
    /// server's [`ShedLog`].
    pub fn try_admit(&self) -> Option<Permit> {
        let Some(server) = self.take_server() else {
            self.shed();
            return None;
        };
        Some(Permit { _server: server, _connection: None })
    }

    /// The server's place, `Some(None)` when it bounds nothing, without counting a refusal.
    fn take_server(&self) -> Option<Option<SemaphoreGuardArc>> {
        match &self.server {
            Some(semaphore) => Some(Some(semaphore.try_acquire_arc()?)),
            None => Some(None),
        }
    }

    fn shed(&self) {
        if let Some(log) = &self.log {
            log.shed();
        }
    }

    /// A connection's bound inside this server's: `None` bounds only by the server.
    pub fn connection(&self, limit: Option<usize>) -> ConnectionAdmission {
        ConnectionAdmission { server: self.clone(), connection: limit.map(|limit| Arc::new(Semaphore::new(limit))) }
    }
}

impl ConnectionAdmission {
    /// A place in both counters, or `None` over either limit, the refusal counted once into the
    /// server's [`ShedLog`].
    pub fn try_admit(&self) -> Option<Permit> {
        let connection = match &self.connection {
            Some(semaphore) => match semaphore.try_acquire_arc() {
                Some(guard) => Some(guard),
                None => {
                    self.server.shed();
                    return None;
                }
            },
            None => None,
        };
        let Some(_server) = self.server.take_server() else {
            self.server.shed();
            return None;
        };
        Some(Permit { _server, _connection: connection })
    }

    pub fn shed_retry_after(&self) -> Duration {
        self.server.retry_after
    }
}

/// The least time between two lines a [`ShedLog`] writes.
pub const SHED_LOG_INTERVAL: Duration = Duration::from_secs(10);

/// The `warn` a server writes when it sheds load: refused calls are counted, and a line is
/// written at most once per [`SHED_LOG_INTERVAL`], carrying in its `shed` field every refusal
/// since the line before. An operator reads it in place of a shed counter, which waits for
/// metrics.
///
/// The first refusal writes a line at once. A refusal within the interval of the last line is
/// counted and writes nothing; the next refusal after the interval writes the line carrying it.
/// Refusals still counted when the last clone of the log drops, with its server, are written
/// then, so each refusal is reported on exactly one line. The line names the transport the log
/// was built with, in `ulo.transport`.
///
/// `Admission::logged` counts an admission's refusals into one; a transport that sheds without
/// an `Admission`, a WebSocket server over `max_connections`, calls [`shed`](Self::shed) itself.
/// Its clock is the `Timer` it is built with, the app's, so a test reading the interval advances
/// a scripted clock rather than waiting.
#[derive(Clone)]
pub struct ShedLog {
    inner: Arc<LogInner>,
}

struct LogInner {
    transport: &'static str,
    /// `dyn Timer` is not `RefUnwindSafe`, which would remove both auto traits from `Admission` and
    /// `ConnectionAdmission`; reading a clock leaves nothing half-updated for an unwind to expose.
    clock: AssertUnwindSafe<Arc<dyn Timer>>,
    window: Mutex<Window>,
}

#[derive(Default)]
struct Window {
    /// When the last line was written; `None` before the first.
    written: Option<Instant>,
    /// Refusals counted since that line and not yet written.
    unwritten: u64,
}

impl ShedLog {
    /// A log naming `transport`, a transport's `Transport::KEY`, timed on `clock`.
    pub fn new(transport: &'static str, clock: Arc<dyn Timer>) -> ShedLog {
        ShedLog { inner: Arc::new(LogInner { transport, clock: AssertUnwindSafe(clock), window: Mutex::new(Window::default()) }) }
    }

    /// Counts one refused call, writing the line when the interval since the last one has passed.
    pub fn shed(&self) {
        let now = self.inner.clock.now();
        let due = {
            let mut window = self.inner.window.lock().unwrap_or_else(PoisonError::into_inner);
            window.unwritten += 1;
            let due = window.written.is_none_or(|written| now.saturating_duration_since(written) >= SHED_LOG_INTERVAL);
            if due {
                window.written = Some(now);
                Some(std::mem::take(&mut window.unwritten))
            } else {
                None
            }
        };
        if let Some(shed) = due {
            write(self.inner.transport, shed);
        }
    }
}

impl Drop for LogInner {
    fn drop(&mut self) {
        let window = self.window.get_mut().unwrap_or_else(PoisonError::into_inner);
        if window.unwritten > 0 {
            write(self.transport, window.unwritten);
        }
    }
}

fn write(transport: &'static str, shed: u64) {
    tracing::warn!(
        ulo.transport = transport,
        shed,
        "calls refused over the in-flight limit; `shed` counts those since the previous such line, written at most once per 10 s"
    );
}

#[cfg(test)]
mod tests {
    use std::panic::{RefUnwindSafe, UnwindSafe};
    use std::sync::{Arc, Mutex, PoisonError};
    use std::time::Duration;

    use tracing::field::{Field, Visit};
    use tracing::span::{Attributes, Id, Record};
    use tracing::{Event, Metadata};
    use ulo::testing::ScriptedTimer;

    use super::{Admission, SHED_LOG_INTERVAL, ShedLog};

    /// Every `warn` written, as its `ulo.transport` and `shed` fields.
    #[derive(Clone, Default)]
    struct Lines(Arc<Mutex<Vec<(String, u64)>>>);

    impl Lines {
        fn taken(&self) -> Vec<(String, u64)> {
            std::mem::take(&mut *self.0.lock().unwrap_or_else(PoisonError::into_inner))
        }
    }

    #[derive(Default)]
    struct Line {
        transport: String,
        shed: Option<u64>,
    }

    impl Visit for Line {
        fn record_str(&mut self, field: &Field, value: &str) {
            if field.name() == "ulo.transport" {
                self.transport = value.to_owned();
            }
        }

        fn record_u64(&mut self, field: &Field, value: u64) {
            if field.name() == "shed" {
                self.shed = Some(value);
            }
        }

        fn record_debug(&mut self, _: &Field, _: &dyn std::fmt::Debug) {}
    }

    impl tracing::Subscriber for Lines {
        fn enabled(&self, _: &Metadata<'_>) -> bool {
            true
        }
        fn new_span(&self, _: &Attributes<'_>) -> Id {
            Id::from_u64(1)
        }
        fn record(&self, _: &Id, _: &Record<'_>) {}
        fn record_follows_from(&self, _: &Id, _: &Id) {}
        fn event(&self, event: &Event<'_>) {
            let mut line = Line::default();
            event.record(&mut line);
            if let Some(shed) = line.shed {
                self.0.lock().unwrap_or_else(PoisonError::into_inner).push((line.transport, shed));
            }
        }
        fn enter(&self, _: &Id) {}
        fn exit(&self, _: &Id) {}
    }

    fn recorded(run: impl FnOnce()) -> Vec<(String, u64)> {
        let lines = Lines::default();
        tracing::subscriber::with_default(lines.clone(), run);
        lines.taken()
    }

    /// A published auto trait is part of the API: a shed log inside an admission must not remove it.
    #[test]
    fn admission_and_the_log_stay_unwind_safe() {
        fn needs<T: UnwindSafe + RefUnwindSafe>() {}
        needs::<Admission>();
        needs::<super::ConnectionAdmission>();
        needs::<ShedLog>();
    }

    fn log(clock: &ScriptedTimer) -> ShedLog {
        ShedLog::new("http", Arc::new(clock.clone()))
    }

    #[test]
    fn the_first_refusal_writes_a_line_at_once_naming_the_transport() {
        let clock = ScriptedTimer::new();
        let log = log(&clock);
        let lines = recorded(|| log.shed());
        assert_eq!(lines, vec![("http".to_owned(), 1)]);
    }

    #[test]
    fn refusals_within_the_interval_are_written_on_the_next_line_after_it() {
        let clock = ScriptedTimer::new();
        let log = log(&clock);
        let lines = recorded(|| {
            log.shed();
            for _ in 0..4 {
                clock.advance(Duration::from_secs(2));
                log.shed();
            }
            clock.advance(SHED_LOG_INTERVAL - Duration::from_secs(8) - Duration::from_millis(1));
            log.shed();
            clock.advance(Duration::from_millis(1));
            log.shed();
        });
        assert_eq!(lines, vec![("http".to_owned(), 1), ("http".to_owned(), 6)], "one line at 0 s, the next at exactly 10 s carrying all six");
    }

    #[test]
    fn a_window_is_timed_from_the_last_line_not_from_the_first_refusal() {
        let clock = ScriptedTimer::new();
        let log = log(&clock);
        let lines = recorded(|| {
            log.shed();
            clock.advance(Duration::from_secs(25));
            log.shed();
            clock.advance(Duration::from_secs(9));
            log.shed();
            clock.advance(Duration::from_secs(1));
            log.shed();
        });
        assert_eq!(lines, vec![("http".to_owned(), 1), ("http".to_owned(), 1), ("http".to_owned(), 2)]);
    }

    #[test]
    fn refusals_unwritten_when_the_last_clone_drops_are_written_then() {
        let clock = ScriptedTimer::new();
        let log = log(&clock);
        let lines = recorded(|| {
            log.shed();
            let clone = log.clone();
            clone.shed();
            clone.shed();
            drop(log);
            drop(clone);
        });
        assert_eq!(lines, vec![("http".to_owned(), 1), ("http".to_owned(), 2)]);
    }

    #[test]
    fn a_log_with_nothing_unwritten_writes_nothing_when_it_drops() {
        let clock = ScriptedTimer::new();
        let lines = recorded(|| {
            let log = log(&clock);
            log.shed();
            drop(log);
            drop(ShedLog::new("rpc", Arc::new(clock.clone())));
        });
        assert_eq!(lines, vec![("http".to_owned(), 1)]);
    }

    /// The law the counting keeps under any interleaving: the lines together carry every refusal
    /// once. Eight threads shed while the clock moves; the sum of `shed` over every line written,
    /// the final drop's included, is the number of refusals.
    #[test]
    fn every_refusal_is_written_on_exactly_one_line_under_concurrent_shedding() {
        let clock = ScriptedTimer::new();
        let log = log(&clock);
        let dispatch = tracing::Dispatch::new(Lines::default());
        let lines = dispatch.downcast_ref::<Lines>().cloned().expect("the dispatch holds the subscriber");
        std::thread::scope(|scope| {
            for _ in 0..8 {
                let (log, dispatch) = (log.clone(), dispatch.clone());
                scope.spawn(move || {
                    tracing::dispatcher::with_default(&dispatch, || {
                        for _ in 0..1_000 {
                            log.shed();
                        }
                    });
                });
            }
            let clock = clock.clone();
            scope.spawn(move || {
                for _ in 0..50 {
                    clock.advance(Duration::from_secs(3));
                    std::thread::yield_now();
                }
            });
        });
        tracing::dispatcher::with_default(&dispatch, || drop(log));
        let written = lines.taken();
        assert_eq!(written.iter().map(|(_, shed)| shed).sum::<u64>(), 8_000, "{written:?}");
        assert!(written.iter().all(|(transport, shed)| transport == "http" && *shed > 0), "{written:?}");
    }

    #[test]
    fn an_admission_counts_its_refusals_and_not_its_admissions() {
        let clock = ScriptedTimer::new();
        let admission = Admission::new(Some(1)).logged(log(&clock));
        let lines = recorded(|| {
            let held = admission.try_admit().expect("the first call is admitted");
            assert!(admission.try_admit().is_none(), "the second call is admitted over a bound of one");
            assert!(admission.try_admit().is_none(), "the third call is admitted over a bound of one");
            drop(held);
            let _again = admission.try_admit().expect("a freed place admits");
            drop(admission);
        });
        assert_eq!(lines, vec![("http".to_owned(), 1), ("http".to_owned(), 1)], "the first refusal at once, the second when the log drops");
    }

    #[test]
    fn a_connections_refusal_is_counted_once_into_its_servers_log() {
        let clock = ScriptedTimer::new();
        let server = Admission::new(Some(2)).logged(log(&clock));
        let lines = recorded(|| {
            let connection = server.connection(Some(1));
            let _held = connection.try_admit().expect("the first call is admitted");
            assert!(connection.try_admit().is_none(), "the connection's bound admitted a second call");
            let other = server.connection(None);
            let _second = other.try_admit().expect("the server's second place");
            assert!(other.try_admit().is_none(), "the server's bound admitted a third call");
            clock.advance(SHED_LOG_INTERVAL);
            assert!(other.try_admit().is_none(), "the server's bound admitted a third call");
        });
        assert_eq!(lines, vec![("http".to_owned(), 1), ("http".to_owned(), 2)]);
    }

    #[test]
    fn an_admission_with_no_log_writes_nothing() {
        let lines = recorded(|| {
            let admission = Admission::new(Some(0));
            assert!(admission.try_admit().is_none());
            drop(admission);
        });
        assert_eq!(lines, Vec::<(String, u64)>::new());
    }
}
