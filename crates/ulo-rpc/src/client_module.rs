//! `RpcClientModule`: binds an `RpcClient` over one link (transports DESIGN §5.4).

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use ulo::{Bound, BoxError, Dep, Module, ModuleDef, ModuleIdentity, Runtime};
use ulo_transport::Count;

use crate::client::{RpcClient, UnusableLink, ZeroCount, ZeroTimeout};
use crate::link::Link;

/// Binds `RpcClient` over link `L`, exported, usually keyed so two clients coexist:
///
/// ```
/// # use std::time::Duration;
/// # use ulo::{Bound, module};
/// # use ulo_rpc::RpcClientModule;
/// # struct Billing;
/// #[module(imports = [RpcClientModule::for_root(ulo_rpc_tcp::Tcp::new("10.0.0.5:7000"))
///     .timeout(Bound::After(Duration::from_secs(2)))
///     .keyed::<Billing>()])]
/// pub struct OrdersModule;
/// ```
///
/// The link connects lazily, on the client's first call, so `connect` does no network I/O for it.
/// The module's init hook calls the link's [`Link::usable`], so a link that can never connect, a
/// tokio-based one built outside a runtime and given none, fails `connect` as [`UnusableLink`]
/// naming `.with_handle(..)`, as a server's link fails `listen()`.
/// The client spawns its tasks on the app's runtime, which also times its calls, reading it as
/// `Dep<dyn Runtime>`: an app without one, `.timer(..)` alone included, fails `wire()` naming the
/// missing binding and `.runtime(..)`. The binding's `on_destroy` hook calls the link's `close`,
/// so the client's connection ends with the app rather than when the runtime drops it; a call
/// made after that connects again.
///
/// Each `for_root` is its own module: two clients of one link type are two imports, never one
/// deduplicated, and diagnostics name either `RpcClientModule`.
pub struct RpcClientModule<L: Link> {
    pub(crate) link: Arc<L>,
    pub(crate) timeout: Bound,
    pub(crate) idle: Bound,
    pub(crate) queued: Count,
    pub(crate) instance: u64,
}

/// What tells two `for_root` calls apart.
#[derive(Clone, PartialEq, Eq, Hash)]
struct Instance(u64);

static INSTANCES: AtomicU64 = AtomicU64::new(0);

impl<L: Link> RpcClientModule<L> {
    pub fn for_root(link: L) -> Self {
        RpcClientModule {
            link: Arc::new(link),
            timeout: Bound::Default,
            idle: Bound::Default,
            queued: Count::Default,
            instance: INSTANCES.fetch_add(1, Ordering::Relaxed),
        }
    }

    /// The client's timeout, as [`RpcClient::timeout`] sets it: the whole-call bound of every
    /// `request` and `send_stream` that sets none of its own, sent as `deadline-ms`, and the bound
    /// on every `emit`'s publish. Five seconds at `Bound::Default`, `Bound::Unbounded` for none.
    /// A call made `.within` an execution is sent the shorter of the execution's remaining time
    /// and this timeout, so a caller with thirty seconds left reaches the callee with five at the
    /// default. It does not apply to a `stream` or `duplex` call, which is bounded only by its own
    /// `idle_timeout` and `deadline`. `Bound::After(Duration::ZERO)` is refused when the app wires,
    /// as [`ZeroTimeout`].
    pub fn timeout(mut self, timeout: Bound) -> Self {
        self.timeout = timeout;
        self
    }

    /// The client's idle timeout, as [`RpcClient::idle_timeout`] sets it: the longest silence
    /// between reply frames of every `stream` and `duplex` call that leaves its own at
    /// `Bound::Default`. None at `Bound::Default` and `Bound::Unbounded`. A lost connection fails
    /// a waiting stream `Unavailable` whatever the idle timeout. `Bound::After(Duration::ZERO)` is
    /// refused when the app wires, as [`ZeroTimeout`].
    pub fn idle_timeout(mut self, idle: Bound) -> Self {
        self.idle = idle;
        self
    }

    /// The client's queue limit, as [`RpcClient::max_queued_items`] sets it: the reply items of
    /// one `stream` or `duplex` call held unread for its caller, 1,024 at `Count::Default`, none at
    /// `Count::Unlimited`. The item past it ends the call `Unavailable` with the reason
    /// `"overflow"`, after the client sends its server `cancel`. `Count::Max(0)` is refused when the
    /// app wires, as [`ZeroCount`].
    pub fn max_queued_items(mut self, items: Count) -> Self {
        self.queued = items;
        self
    }
}

impl<L: Link> Module for RpcClientModule<L> {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_value(&Instance(self.instance)).label("RpcClientModule")
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        let zero = Bound::After(Duration::ZERO);
        let refused: Option<BoxError> = if self.timeout == zero {
            Some(ZeroTimeout::of_module("RpcClientModule::timeout", L::NAME, "every call").into())
        } else if self.idle == zero {
            Some(ZeroTimeout::of_module("RpcClientModule::idle_timeout", L::NAME, "every stream").into())
        } else if self.queued == Count::Max(0) {
            Some(ZeroCount::new("RpcClientModule::max_queued_items", L::NAME).into())
        } else {
            None
        };
        if let Some(refusal) = refused {
            // A module has no `prepare`; the refusal is recorded for `wire()` to report beside every
            // other wiring error.
            m.try_value::<RpcClient, _>(Err(refusal));
        } else {
            let link = Arc::clone(&self.link);
            let closing = Arc::clone(&self.link);
            let (timeout, idle, queued) = (self.timeout, self.idle, self.queued);
            let checked = Arc::clone(&self.link);
            m.singleton(move |runtime: Dep<dyn Runtime>| {
                let link = Arc::clone(&link);
                async move { RpcClient::of_module(link, timeout, idle, runtime.into_arc()).with_queued(queued) }
            })
            .on_destroy(move || {
                let link = Arc::clone(&closing);
                async move {
                    if let Err(error) = link.close().await {
                        tracing::warn!(%error, link = L::NAME, "the RPC client's link did not close cleanly");
                    }
                }
            });
            m.on_init(move || {
                let usable = checked.usable().map_err(|source| UnusableLink::of_module(L::NAME, source));
                async move { usable }
            });
        }
        m.export::<RpcClient>();
    }
}
