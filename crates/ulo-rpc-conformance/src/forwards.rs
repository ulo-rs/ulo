//! A check that a link wrapping another forwards what the server asks of a link before any I/O.

use std::fmt;

use ulo::{App, BoxError, Module, ModuleDef, ModuleIdentity};
use ulo_rpc::{Instances, Link, Pattern, Subscription};
use ulo_transport::Count;

/// Asserts that `wrapper`, a link wrapping another, answers what `inner`, a link configured as
/// the one it wraps, answers to every method the server calls before any I/O: `capabilities`,
/// `usable`, `subscribable` over patterns a broker commonly refuses, `endpoint`, `bound` and
/// `prepare`, called on both against one app with no runtime and no timer after
/// `max_inflight` is set on both. The suite cannot tell that a link wraps another, so a link
/// author calls this from the link's own tests, once per configuration of the inner link worth
/// checking. Panics naming the first method whose answers differ.
///
/// A trait method the wrapper does not write answers the trait's default, so a missing forward
/// shows only where the inner link's answer is not that default: an inner link configured to
/// refuse (a zero bound `usable` refuses, a pattern its broker cannot name) shows `usable` and
/// `subscribable`, and one naming its endpoint shows `endpoint`. `max_inflight` answers nothing,
/// so whether the wrapper forwards it is not observable here: a wrapper generic over the link it
/// wraps shows it by wrapping a link that records the call.
///
/// ```
/// # use std::future::Future;
/// # use std::sync::Arc;
/// # use ulo::{AppHandle, BoundAddr, BoxError, Runtime};
/// # use ulo_rpc::link::Inbound;
/// # use ulo_rpc::{Capabilities, Link, Outbound, Subscription};
/// # use ulo_transport::Count;
/// /// A TCP link whose server logs each delivery: everything else is the wrapped link's.
/// struct Logged(ulo_rpc_tcp::Tcp);
///
/// impl Link for Logged {
///     const NAME: &'static str = ulo_rpc_tcp::Tcp::NAME;
///     fn capabilities(&self) -> Capabilities { self.0.capabilities() }
///     fn prepare(&mut self, app: &AppHandle) -> impl Future<Output = Result<(), BoxError>> + Send { self.0.prepare(app) }
///     fn usable(&self) -> Result<(), BoxError> { self.0.usable() }
///     fn max_inflight(&mut self, calls: Count) { self.0.max_inflight(calls) }
///     fn subscribable(&self, subscriptions: &[Subscription]) -> Result<(), BoxError> { self.0.subscribable(subscriptions) }
///     fn listen(&self, subscriptions: &[Subscription]) -> impl Future<Output = Result<Inbound, BoxError>> + Send { self.0.listen(subscriptions) }
///     fn connect(&self, runtime: &Arc<dyn Runtime>) -> impl Future<Output = Result<Outbound, BoxError>> + Send { self.0.connect(runtime) }
///     fn drain(&self) -> impl Future<Output = ()> + Send { self.0.drain() }
///     fn close(&self) -> impl Future<Output = Result<(), BoxError>> + Send { self.0.close() }
///     fn bound(&self) -> Vec<BoundAddr> { self.0.bound() }
///     fn endpoint(&self) -> Option<String> { self.0.endpoint() }
/// }
///
/// let tcp = || ulo_rpc_tcp::Tcp::new("127.0.0.1:7000");
/// futures_executor::block_on(ulo_rpc_conformance::assert_forwards(&mut Logged(tcp()), &mut tcp()));
/// ```
pub async fn assert_forwards<W: Link, L: Link>(wrapper: &mut W, inner: &mut L) {
    same("capabilities", wrapper.capabilities(), inner.capabilities());
    same("usable", answer(wrapper.usable()), answer(inner.usable()));
    for subscriptions in refused_by_some() {
        let what = format!("subscribable over {:?}", subscriptions.iter().map(|subscription| subscription.pattern.as_str()).collect::<Vec<_>>());
        same(&what, answer(wrapper.subscribable(&subscriptions)), answer(inner.subscribable(&subscriptions)));
    }
    same("endpoint", wrapper.endpoint(), inner.endpoint());
    same("bound", wrapper.bound(), inner.bound());
    let calls = Count::Max(7);
    wrapper.max_inflight(calls);
    inner.max_inflight(calls);
    let app = App::builder(Bare).wire().expect("an empty app wires").connect().await.expect("an empty app connects");
    let (wrapped, unwrapped) = (wrapper.prepare(&app.handle()).await, inner.prepare(&app.handle()).await);
    same("prepare", answer(wrapped), answer(unwrapped));
}

/// What a method answered, as two answers are compared: success, or the refusal's text.
fn answer(outcome: Result<(), BoxError>) -> Result<(), String> {
    outcome.map_err(|refused| refused.to_string())
}

fn same<T: PartialEq + fmt::Debug>(what: &str, wrapper: T, inner: T) {
    assert!(wrapper == inner, "the wrapper's `{what}` answers {wrapper:?} where the link it wraps answers {inner:?}: it does not forward `{what}`");
}

/// Subscription lists one broker or another refuses, each alone and then all at once: names past
/// AMQP's and Kafka's limits, characters a topic, subject or channel name cannot hold, and the
/// wildcards and shared-subscription prefixes some brokers read in a filter.
fn refused_by_some() -> Vec<Vec<Subscription>> {
    let patterns = [
        "conformance.add".to_owned(),
        String::new(),
        ".".to_owned(),
        "a/b".to_owned(),
        "a:b".to_owned(),
        "a b".to_owned(),
        "#".to_owned(),
        "+".to_owned(),
        "*".to_owned(),
        ">".to_owned(),
        "$share/g/a".to_owned(),
        "caf\u{e9}".to_owned(),
        "p".repeat(242),
        "p".repeat(250),
        "p".repeat(256),
    ];
    let mut lists: Vec<Vec<Subscription>> = Vec::new();
    for pattern in &patterns {
        for instances in [Instances::One, Instances::Every] {
            lists.push(vec![Subscription::new(pattern.as_str(), instances)]);
        }
    }
    lists.push(patterns.iter().map(|pattern| Subscription::from(Pattern::from(pattern.as_str()))).collect());
    lists
}

/// The app `prepare` is called against: no module, no runtime and no timer.
struct Bare;

impl Module for Bare {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        let _ = m;
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::panic::AssertUnwindSafe;
    use std::sync::Arc;

    use ulo::{AppHandle, Runtime};
    use ulo_rpc::link::Inbound;
    use ulo_rpc::{Capabilities, Codec, DeliveryMode, Outbound};

    use super::*;

    /// A link answering something other than the trait's default to every method `assert_forwards`
    /// compares, so a wrapper that leaves one unwritten answers differently there.
    pub(crate) struct Refuses;

    impl Link for Refuses {
        const NAME: &'static str = "refuses";

        fn capabilities(&self) -> Capabilities {
            Capabilities::new(DeliveryMode::FanOut).codec(Codec::Cbor).max_frame(Some(7))
        }

        async fn prepare(&mut self, _app: &AppHandle) -> Result<(), BoxError> {
            Err("this link refuses `prepare`".into())
        }

        fn usable(&self) -> Result<(), BoxError> {
            Err("this link refuses `usable`".into())
        }

        fn subscribable(&self, subscriptions: &[Subscription]) -> Result<(), BoxError> {
            match subscriptions.iter().find(|subscription| subscription.pattern.as_str().contains('/')) {
                Some(subscription) => Err(format!("this link refuses the pattern `{}`", subscription.pattern).into()),
                None => Ok(()),
            }
        }

        async fn listen(&self, _subscriptions: &[Subscription]) -> Result<Inbound, BoxError> {
            Err("this link has no server side".into())
        }

        async fn connect(&self, _runtime: &Arc<dyn Runtime>) -> Result<Outbound, BoxError> {
            Err("this link has no client side".into())
        }

        async fn drain(&self) {}

        async fn close(&self) -> Result<(), BoxError> {
            Ok(())
        }

        fn bound(&self) -> Vec<ulo::BoundAddr> {
            vec![ulo::BoundAddr::new("rpc", std::net::SocketAddr::from(([127, 0, 0, 1], 1)))]
        }

        fn endpoint(&self) -> Option<String> {
            Some("refuses.example:1".to_owned())
        }
    }

    /// The method a [`Skips`] wrapper leaves to the trait's default.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Skipped {
        Capabilities,
        Prepare,
        Usable,
        Subscribable,
        Endpoint,
        Bound,
    }

    /// A wrapper forwarding every method but one.
    struct Skips {
        link: Refuses,
        skipped: Skipped,
    }

    impl Link for Skips {
        const NAME: &'static str = Refuses::NAME;

        fn capabilities(&self) -> Capabilities {
            if self.skipped == Skipped::Capabilities { Capabilities::new(DeliveryMode::Competing) } else { self.link.capabilities() }
        }

        async fn prepare(&mut self, app: &AppHandle) -> Result<(), BoxError> {
            if self.skipped == Skipped::Prepare { Ok(()) } else { self.link.prepare(app).await }
        }

        fn usable(&self) -> Result<(), BoxError> {
            if self.skipped == Skipped::Usable { Ok(()) } else { self.link.usable() }
        }

        fn subscribable(&self, subscriptions: &[Subscription]) -> Result<(), BoxError> {
            if self.skipped == Skipped::Subscribable { Ok(()) } else { self.link.subscribable(subscriptions) }
        }

        fn listen(&self, subscriptions: &[Subscription]) -> impl Future<Output = Result<Inbound, BoxError>> + Send {
            self.link.listen(subscriptions)
        }

        fn connect(&self, runtime: &Arc<dyn Runtime>) -> impl Future<Output = Result<Outbound, BoxError>> + Send {
            self.link.connect(runtime)
        }

        fn drain(&self) -> impl Future<Output = ()> + Send {
            self.link.drain()
        }

        fn close(&self) -> impl Future<Output = Result<(), BoxError>> + Send {
            self.link.close()
        }

        fn bound(&self) -> Vec<ulo::BoundAddr> {
            if self.skipped == Skipped::Bound { Vec::new() } else { self.link.bound() }
        }

        fn endpoint(&self) -> Option<String> {
            if self.skipped == Skipped::Endpoint { None } else { self.link.endpoint() }
        }
    }

    /// The panic `assert_forwards` raises over `wrapper`, `None` when it passes.
    fn failure<W: Link>(mut wrapper: W) -> Option<String> {
        let checked = std::panic::catch_unwind(AssertUnwindSafe(|| futures_executor::block_on(assert_forwards(&mut wrapper, &mut Refuses))));
        checked.err().map(|panic| match panic.downcast::<String>() {
            Ok(message) => *message,
            Err(panic) => panic.downcast::<&str>().map_or_else(|_| "a panic of another type".to_owned(), |message| (*message).to_owned()),
        })
    }

    /// Each method a wrapper leaves unwritten is named, and a wrapper writing them all passes.
    /// Planted breaks: `assert_forwards` comparing `subscribable` over the first pattern alone,
    /// which every link accepts; `bound` left uncompared.
    #[test]
    fn a_wrapper_leaving_a_method_unwritten_is_named() {
        for (skipped, method) in [
            (Skipped::Capabilities, "`capabilities`"),
            (Skipped::Prepare, "`prepare`"),
            (Skipped::Usable, "`usable`"),
            (Skipped::Subscribable, "`subscribable over"),
            (Skipped::Endpoint, "`endpoint`"),
            (Skipped::Bound, "`bound`"),
        ] {
            let message = failure(Skips { link: Refuses, skipped }).unwrap_or_else(|| panic!("a wrapper skipping {skipped:?} passed"));
            assert!(message.contains(&format!("wrapper's {method}")), "skipping {skipped:?}: {message}");
        }
    }

    /// A wrapper that forwards every method passes, the inner link answering refusals throughout.
    #[test]
    fn a_wrapper_forwarding_every_method_passes() {
        struct Forwards(Refuses);

        impl Link for Forwards {
            const NAME: &'static str = Refuses::NAME;

            fn capabilities(&self) -> Capabilities {
                self.0.capabilities()
            }

            fn prepare(&mut self, app: &AppHandle) -> impl Future<Output = Result<(), BoxError>> + Send {
                self.0.prepare(app)
            }

            fn usable(&self) -> Result<(), BoxError> {
                self.0.usable()
            }

            fn max_inflight(&mut self, calls: Count) {
                self.0.max_inflight(calls);
            }

            fn subscribable(&self, subscriptions: &[Subscription]) -> Result<(), BoxError> {
                self.0.subscribable(subscriptions)
            }

            fn listen(&self, subscriptions: &[Subscription]) -> impl Future<Output = Result<Inbound, BoxError>> + Send {
                self.0.listen(subscriptions)
            }

            fn connect(&self, runtime: &Arc<dyn Runtime>) -> impl Future<Output = Result<Outbound, BoxError>> + Send {
                self.0.connect(runtime)
            }

            fn drain(&self) -> impl Future<Output = ()> + Send {
                self.0.drain()
            }

            fn close(&self) -> impl Future<Output = Result<(), BoxError>> + Send {
                self.0.close()
            }

            fn bound(&self) -> Vec<ulo::BoundAddr> {
                self.0.bound()
            }

            fn endpoint(&self) -> Option<String> {
                self.0.endpoint()
            }
        }

        assert_eq!(failure(Forwards(Refuses)), None);
    }
}
