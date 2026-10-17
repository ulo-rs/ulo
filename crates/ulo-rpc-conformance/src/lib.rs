//! The conformance suite every `ulo-rpc` link runs (transports DESIGN §5.2): one scenario list,
//! stamped per link by [`conformance_suite!`] from a [`Broker`] the link crate implements in its
//! `tests/`.
//!
//! ```ignore
//! // crates/ulo-rpc-nats/tests/conformance.rs
//! struct NatsBroker { url: String }
//!
//! impl ulo_rpc_conformance::Broker for NatsBroker {
//!     type Harness = ulo_rpc_conformance::OnTokio;
//!     type Link = ulo_rpc_nats::Nats;
//!     async fn start() -> Self { /* a fresh broker or subject space */ }
//!     fn link(&self) -> ulo_rpc_nats::Nats { ulo_rpc_nats::Nats::url(&self.url) }
//!     async fn disrupt(&self) { /* sever the connection */ }
//! }
//!
//! ulo_rpc_conformance::conformance_suite!(NatsBroker);
//! ```
//!
//! A scenario asserts kinds and reason strings, never message text. A scenario a link's
//! capabilities exclude asserts the refusal the capability declares instead: a streamed shape on
//! UDP is refused at startup, a binary payload on a JSON link before any I/O and its handler at
//! startup, a miss on a link without `miss_signal` is the client's `Timeout`, and the
//! two-instance scenario runs one instance on an `Addressed` link, where a second cannot share the
//! address.
//!
//! No scenario passes on silence. The client's own `Timeout` passes only where it is the declared
//! answer: a scenario about the client's timeout or `deadline-ms`, a call nothing takes on a link
//! without `miss_signal`, a call reaching a draining server on a link that declares
//! `holds_unserved`, where an event emitted then has to reach the next instance, and a call sent
//! during the drain on a link that declares `confirms_drain(false)`, where a call sent once the
//! server has closed has to be refused `Unavailable`; a link that does not declare it is held to the
//! confirmed drain. A scenario that cannot apply to a link is declared not applicable
//! in [`conformance_suite!`] and reported as ignored; run on that link, it fails.
//!
//! The suite names no runtime. A broker's [`Harness`] is its runtime half, as the HTTP and runtime
//! suites' are: the app's runtime, which every server and client in a scenario runs on and every
//! wait and task of the scenario's own goes through, and a `block_on` each scenario runs inside.
//! `OnTokio`, behind the `tokio` feature, is the harness of every link built on tokio; `OnSmol`,
//! behind `smol`, runs the scenarios on an `async-executor` `Executor`. The [`relay`] runs on
//! threads of its own, on `std` sockets, so it carries a link on either.
//!
//! A host short of memory bounds how many scenarios hold an environment at once by setting
//! [`PARALLEL_VAR`], `ULO_CONFORMANCE_PARALLEL`, to a positive integer: the bound is the smaller of
//! it and the broker's own [`Broker::PARALLEL`], or it alone when the broker declares none. Unset
//! or empty, it bounds nothing, and libtest's `--test-threads` still applies on top of either.
//!
//! ```text
//! ULO_CONFORMANCE_PARALLEL=6 cargo test -p ulo-rpc-rabbitmq --features integration --test conformance
//! ```

use std::error::Error;
use std::future::Future;
use std::num::NonZeroUsize;
use std::time::Duration;

use ulo::{BoundAddr, Runtime};
use ulo_rpc::Link;
use ulo_rpc::__private::ClientProbe;

use crate::relay::Outage;

pub mod cases;
mod failures;
pub mod relay;
mod rt;
#[cfg(feature = "smol")]
mod smol_harness;
#[cfg(feature = "tokio")]
mod tokio_harness;

pub use cases::deadlines::Stamped;
pub use failures::failures_dir;
#[cfg(feature = "smol")]
pub use smol_harness::OnSmol;
#[cfg(feature = "tokio")]
pub use tokio_harness::OnTokio;

/// The environment variable through which a host bounds how many scenarios of one suite hold an
/// environment at once. A value that is not a positive integer fails every scenario through
/// [`startup_failed!`].
pub const PARALLEL_VAR: &str = "ULO_CONFORMANCE_PARALLEL";

/// `error` and every source under it, joined by `: `, for a failure message: a scenario that fails
/// while its environment starts says what failed, the broker's container, the server's bind or the
/// client's connect, and why. A source whose text the message already carries is not repeated.
pub fn report(error: &(dyn Error + 'static)) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        let line = cause.to_string();
        if !text.contains(&line) {
            text.push_str(": ");
            text.push_str(&line);
        }
        source = cause.source();
    }
    text
}

/// Fails a scenario whose environment did not start, as `panic!` with the same arguments would,
/// after writing the message to a file of its own under [`failures_dir`]; the panic names the file.
/// Every startup path in the suite fails through it, and a [`Broker`] implementation's own startup
/// should too.
#[macro_export]
macro_rules! startup_failed {
    ($($arg:tt)+) => {
        $crate::__private::startup_failed(::std::format!($($arg)+))
    };
}

/// The runtime a broker's scenarios run on, as the suite drives it: the app's runtime and the
/// executor each scenario is run to its end on.
pub trait Harness: Send + Sync + 'static {
    /// The app's runtime: every server and client a scenario starts runs on it, and the scenario's
    /// own sleeps, timeouts and tasks go through it.
    type Runtime: Runtime;

    /// A runtime value, taken wherever a scenario needs one: on the thread `block_on` runs and on
    /// every thread running the tasks the runtime spawns while it does.
    fn runtime() -> Self::Runtime;

    /// Runs `fut` to its end from a plain `#[test]` thread, on an executor that runs the tasks the
    /// runtime spawns while `fut` waits. Each scenario calls it once.
    fn block_on<F: Future>(fut: F) -> F::Output;
}

/// One link's environment for the suite: a broker, or, for TCP and UDP, the server's own socket,
/// bound at port 0 and read back.
pub trait Broker: Sized + Send + Sync + 'static {
    /// The runtime the scenarios run on: `OnTokio`, behind the `tokio` feature, for a link built on
    /// tokio.
    type Harness: Harness;

    type Link: Link;

    /// How many scenarios hold an environment at once; `None`, the default, leaves the scenarios
    /// to libtest's parallelism. An environment heavy enough that starting one per scenario at
    /// once starves any host, such as a broker container per scenario, bounds it: the stamped
    /// tests then wait for a slot before `start` and keep it until the scenario has returned and
    /// dropped its environment. What one host's memory allows is not the broker's to declare;
    /// that host sets [`PARALLEL_VAR`], and the smaller of the two applies.
    const PARALLEL: Option<NonZeroUsize> = None;

    /// Starts the environment for one scenario, so no state leaks between scenarios. Every
    /// scenario uses the same patterns, and on a broker the same default group and control lane,
    /// and the stamped tests run in parallel, up to the bound [`PARALLEL`](Self::PARALLEL) and
    /// [`PARALLEL_VAR`] set, so each
    /// start answers a broker or a namespace no other scenario shares: a fresh container, or, on
    /// TCP and UDP, a server link on port 0, whose port the OS chooses at the server's bind and
    /// [`client_link`](Self::client_link) receives. A container reached at a port published on the host is started
    /// through [`relay::unshadowed`], so a host listener on the same port does not answer in its
    /// place, and a start that fails does so through [`startup_failed!`].
    fn start() -> impl Future<Output = Self> + Send;

    /// A link for the server, configured for this environment; called once per server instance,
    /// and for the link's `capabilities`, which the scenarios read to choose what they assert.
    /// Every link it answers declares the same capabilities. A link that binds a socket binds port
    /// 0, so no two servers, and no other process, contend for one the suite chose.
    fn link(&self) -> Self::Link;

    /// A link for the client, called once per client, after the scenario's servers are bound:
    /// `server` holds every address they bound, `App<Bound>::addresses()` in the order they
    /// started, empty on a broker link, whose servers bind nothing. The server's link unless the
    /// environment puts something between the two, such as a proxy `disrupt` cuts, or the client
    /// has to be given the port the server's bind chose. It declares the capabilities `link` does.
    fn client_link(&self, server: &[BoundAddr]) -> Self::Link {
        let _ = server;
        self.link()
    }

    /// Severs the client's connection however the environment allows, for the recovery scenario:
    /// the calls waiting on it fail `Unavailable`, and the client connects again for the next. On
    /// a link declaring `durable_replies` the waiting call is answered instead, so a `disrupt` that
    /// severed nothing would pass unnoticed: there `disrupt` fails when it closed no connection
    /// (`Relay::cut_for` answers how many), and keeps the client out until the waiting call's
    /// reply is published, three seconds after it begins, which [`outage`](Self::outage) shows.
    fn disrupt(&self) -> impl Future<Output = ()> + Send;

    /// The client's outage from the last `disrupt`, observed: when its connections had closed, and
    /// when the environment first let one through again. A link declaring `durable_replies`
    /// reports it, and the recovery scenario requires the held call's answer to fall inside it, so
    /// the reply is shown to wait for a client that was gone. `None`, the default, elsewhere.
    fn outage(&self) -> Option<Outage> {
        None
    }

    /// How many connections the client's link holds open through the environment, for the
    /// client-close scenario, which requires them to end with the client's app and stay ended:
    /// `Relay::open` counts them where a relay carries the client. `None`, the default, where the
    /// environment cannot count them; UDP holds no connection to count.
    fn client_connections(&self) -> impl Future<Output = Option<usize>> + Send {
        async { None }
    }

    /// How long this environment takes, which a slow broker raises.
    fn budget(&self) -> Budget {
        Budget::default()
    }

    /// The client side of `link` as the held-`cancel` scenarios read it, on a link that holds a
    /// streamed request's `cancel` until the server acknowledges its `open`: NATS, RabbitMQ, MQTT
    /// and Kafka answer the link's own probe. `None`, the default, elsewhere, where those
    /// scenarios are not applicable.
    fn probe(link: &Self::Link) -> Option<&ClientProbe> {
        let _ = link;
        None
    }
}

/// How long a scenario waits on its environment.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Budget {
    /// From `start` until the server answers its first call.
    pub boot: Duration,
    /// For a one-way effect to show: an event handled, a subscription in place.
    pub settle: Duration,
    /// From `disrupt` until calls succeed again.
    pub recovery: Duration,
}

impl Budget {
    pub const fn new(boot: Duration, settle: Duration, recovery: Duration) -> Self {
        Budget { boot, settle, recovery }
    }
}

impl Default for Budget {
    fn default() -> Self {
        Budget::new(Duration::from_secs(5), Duration::from_millis(500), Duration::from_secs(10))
    }
}

/// Stamps every scenario as a `#[test]` for the broker type `$broker`, each running its scenario
/// through the broker's [`Harness::block_on`].
///
/// A scenario that cannot apply to a link is declared with its reason, and stamped
/// `#[ignore = "not applicable: <reason>"]`, so the test report counts it as ignored rather than
/// passed:
///
/// ```ignore
/// ulo_rpc_conformance::conformance_suite!(UdpLoopback; not_applicable {
///     recovery_after_disrupt: "UDP holds no connection to lose",
/// });
/// ```
///
/// A declared name that is no scenario fails to compile. A scenario run on a link it does not
/// apply to fails rather than passing, so a link leaving out a declaration it needs fails too;
/// `cargo test -- --ignored` runs the declared ones, which then fail the same way.
#[macro_export]
macro_rules! conformance_suite {
    ($broker:ty $(,)?) => {
        $crate::conformance_suite!($broker; not_applicable {});
    };
    ($broker:ty; not_applicable { $($skip:ident : $why:literal),* $(,)? } $(,)?) => {
        $crate::conformance_suite!(@stamper [$] $broker; $($skip : $why),*);
    };
    (@stamper [$d:tt] $broker:ty; $($skip:ident : $why:literal),*) => {
        macro_rules! __ulo_rpc_conformance_stamp {
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
        $crate::conformance_suite!(@cases $broker;
            unary_round_trip => unary::round_trip,
            handler_takes_its_context => unary::context_parameter,
            domain_error_envelope => errors::domain_error,
            error_handler_answers_its_own_value => errors::substituted,
            unencodable_reply_is_internal => errors::unencodable_reply,
            guard_refusal_is_forbidden => errors::guard_refusal,
            panic_is_internal => errors::panic,
            undecodable_payload_is_bad_request => errors::undecodable_payload,
            headers_reach_call_headers => headers::reach_call_headers,
            event_reaches_its_handler => events::reaches_handler,
            a_refused_event_is_never_redelivered => events::refused_event_is_not_redelivered,
            an_event_carries_none_of_its_emitter_s_deadline => events::emitter_s_deadline_is_not_the_event_s,
            an_event_emitted_in_a_cancelled_execution_is_not_sent => events::cancelled_before_the_emit_is_not_sent,
            an_event_sent_before_its_emitter_is_cancelled_is_handled => events::cancelled_after_the_send_is_handled,
            unhandled_pattern => misses::unhandled_pattern,
            unhandled_event_is_acknowledged => misses::unhandled_event,
            server_stream_in_order => streams::server_stream,
            client_stream => streams::client_stream,
            client_stream_in_order => streams::client_stream_in_order,
            bidi_stream => streams::bidi_stream,
            cancel_mid_stream => cancel::mid_stream,
            deadline_ms_fires_deadline => deadlines::deadline_ms,
            client_timeout_is_timeout => deadlines::client_timeout,
            a_call_s_timeout_is_the_server_s_deadline => deadlines::timeout_is_the_server_s_deadline,
            a_stream_s_idle_timeout_is_per_frame => deadlines::stream_idle_timeout_is_per_frame,
            a_stream_s_idle_timeout_ends_it_on_silence => deadlines::idle_timeout_ends_a_silent_stream,
            a_stream_s_deadline_is_the_server_s_deadline => deadlines::stream_deadline_is_the_server_s,
            a_stream_with_neither_bound_runs_past_both => deadlines::stream_with_neither_bound_runs_on,
            a_stream_s_unbounded_idle_timeout_outlasts_the_client_s => deadlines::unbounded_idle_timeout_outlasts_the_client_s,
            a_stream_s_default_idle_timeout_is_the_client_s => deadlines::default_idle_timeout_is_the_client_s,
            a_stream_s_shorter_idle_timeout_replaces_the_client_s => deadlines::shorter_idle_timeout_replaces_the_client_s,
            a_stream_s_zero_idle_timeout_is_refused_before_any_frame => deadlines::zero_idle_timeout_is_refused_before_any_frame,
            a_stream_s_longest_idle_timeout_does_not_panic => deadlines::longest_idle_timeout_does_not_panic,
            a_call_s_zero_timeout_is_refused_before_the_server_sees_it => deadlines::zero_call_timeout_is_refused_before_the_server_sees_it,
            a_stream_s_zero_deadline_is_refused_before_the_server_sees_it => deadlines::zero_stream_deadline_is_refused_before_the_server_sees_it,
            a_call_past_its_execution_s_deadline_is_refused_before_the_server_sees_it => deadlines::passed_deadline_is_refused_before_the_server_sees_it,
            a_call_in_a_cancelled_execution_is_refused_before_the_server_sees_it => deadlines::cancelled_execution_s_call_is_refused_before_the_server_sees_it,
            binary_payload => payloads::binary,
            oversized_payload => payloads::oversized,
            drain => drain::drain,
            drain_window => drain::window,
            recovery_after_disrupt => recovery::after_disrupt,
            client_close => client_close::client_close,
            two_instances => delivery::two_instances,
            over_the_bound => admission::over_the_bound,
            client_on_a_thread_without_a_runtime => threads::plain_thread,
            cancel_follows_its_request => order::cancel_follows_its_request,
            cancel_before_opened => order::cancel_before_opened,
            a_late_opened_releases_the_held_cancel => order::late_opened,
            a_held_cancel_is_dropped_when_its_hold_runs_out => order::hold_runs_out,
            a_held_cancel_lasts_until_its_call_s_deadline => order::held_until_deadline,
        );
    };
    (@cases $broker:ty; $($name:ident => $module:ident :: $case:ident),* $(,)?) => {
        static __ULO_RPC_CONFORMANCE_SLOTS: $crate::__private::Slots = $crate::__private::Slots::new();
        $(
            __ulo_rpc_conformance_stamp!($name, fn $name() {
                let _slot = __ULO_RPC_CONFORMANCE_SLOTS.hold(<$broker as $crate::Broker>::PARALLEL);
                <<$broker as $crate::Broker>::Harness as $crate::Harness>::block_on(
                    $crate::cases::$module::$case::<$broker>(),
                );
            });
        )*
    };
}

#[doc(hidden)]
pub mod __private {
    use std::num::NonZeroUsize;

    pub use crate::failures::startup_failed;
    use std::sync::{Condvar, Mutex, OnceLock, PoisonError};

    use crate::PARALLEL_VAR;

    /// The slots one stamped suite's scenarios share. Each scenario runs on its own test thread,
    /// so a slot is waited for by blocking that thread before the scenario's runtime starts.
    pub struct Slots {
        held: Mutex<usize>,
        freed: Condvar,
    }

    impl Slots {
        pub const fn new() -> Self {
            Slots { held: Mutex::new(0), freed: Condvar::new() }
        }

        /// A slot when the broker's `declared` bound or the host's [`PARALLEL_VAR`] bounds the
        /// suite, the smaller when both do, held until the guard drops; `None` otherwise.
        pub fn hold(&'static self, declared: Option<NonZeroUsize>) -> Option<Slot> {
            let parallel = bound(declared, host_parallel())?;
            let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
            while *held >= parallel.get() {
                held = self.freed.wait(held).unwrap_or_else(PoisonError::into_inner);
            }
            *held += 1;
            Some(Slot(self))
        }
    }

    impl Default for Slots {
        fn default() -> Self {
            Slots::new()
        }
    }

    /// One held slot, given back on drop, a scenario's panic included.
    pub struct Slot(&'static Slots);

    impl Drop for Slot {
        fn drop(&mut self) {
            *self.0.held.lock().unwrap_or_else(PoisonError::into_inner) -= 1;
            self.0.freed.notify_one();
        }
    }

    fn bound(declared: Option<NonZeroUsize>, host: Option<NonZeroUsize>) -> Option<NonZeroUsize> {
        match (declared, host) {
            (Some(declared), Some(host)) => Some(declared.min(host)),
            (declared, host) => declared.or(host),
        }
    }

    /// [`PARALLEL_VAR`] as the host set it, read once per test binary: `None` unset or empty.
    fn host_parallel() -> Option<NonZeroUsize> {
        static HOST: OnceLock<Option<NonZeroUsize>> = OnceLock::new();
        *HOST.get_or_init(|| match std::env::var(PARALLEL_VAR) {
            Err(std::env::VarError::NotPresent) => None,
            Ok(value) if value.trim().is_empty() => None,
            Ok(value) => match value.trim().parse::<NonZeroUsize>() {
                Ok(parallel) => Some(parallel),
                Err(_) => startup_failed(format!("{PARALLEL_VAR} is `{value}`, not a positive integer")),
            },
            Err(std::env::VarError::NotUnicode(value)) => startup_failed(format!("{PARALLEL_VAR} is {value:?}, not a positive integer")),
        })
    }

    #[cfg(test)]
    mod tests {
        use std::num::NonZeroUsize;

        use super::bound;

        fn n(value: usize) -> Option<NonZeroUsize> {
            NonZeroUsize::new(value)
        }

        #[test]
        fn the_smaller_of_two_bounds_applies() {
            assert_eq!(bound(n(4), n(6)), n(4), "a broker's bound below the host's");
            assert_eq!(bound(n(6), n(4)), n(4), "a host's bound below the broker's");
        }

        #[test]
        fn either_bound_alone_applies() {
            assert_eq!(bound(n(4), None), n(4), "the broker's bound, the host setting none");
            assert_eq!(bound(None, n(6)), n(6), "the host's bound, the broker declaring none");
            assert_eq!(bound(None, None), None, "neither bound");
        }
    }
}
