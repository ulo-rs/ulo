//! A scenario tied to a capability reads the link's declaration: on a link declared without
//! `ordered_control` the ordered-cancel scenario returns before it starts a link, and on a link
//! declaring nothing it runs. The link here fails every call that starts it, so a run shows as a
//! panic and a skip as a return.

use std::future::Future;
use std::sync::Arc;

use ulo::{BoxError, Runtime};
use ulo_rpc::link::Inbound;
use ulo_rpc::{Capabilities, DeliveryMode, Link, Outbound, Pattern};
use ulo_rpc_conformance::{Broker, Harness, OnTokio};

/// Why [`Unordered`] lacks `ordered_control`.
const REASON: &str = "the test link's `cancel` travels a lane of its own";

/// A link that refuses to be started, declaring `ordered_control` or not.
struct Refusing {
    ordered: bool,
}

impl Link for Refusing {
    const NAME: &'static str = "refusing";

    fn capabilities(&self) -> Capabilities {
        let capabilities = Capabilities::new(DeliveryMode::Addressed);
        if self.ordered { capabilities } else { capabilities.without_ordered_control(REASON) }
    }

    fn listen(&self, _: &[Pattern]) -> impl Future<Output = Result<Inbound, BoxError>> + Send {
        async { Err(BoxError::from("the scenario started a server on the link")) }
    }

    fn connect(&self, _: &Arc<dyn Runtime>) -> impl Future<Output = Result<Outbound, BoxError>> + Send {
        async { Err(BoxError::from("the scenario started a client on the link")) }
    }

    fn drain(&self) -> impl Future<Output = ()> + Send {
        async {}
    }

    fn close(&self) -> impl Future<Output = Result<(), BoxError>> + Send {
        async { Ok(()) }
    }
}

struct Unordered;

impl Broker for Unordered {
    type Harness = OnTokio;
    type Link = Refusing;

    async fn start() -> Self {
        Unordered
    }

    fn link(&self) -> Refusing {
        Refusing { ordered: false }
    }

    async fn disrupt(&self) {}
}

struct Ordered;

impl Broker for Ordered {
    type Harness = OnTokio;
    type Link = Refusing;

    async fn start() -> Self {
        Ordered
    }

    fn link(&self) -> Refusing {
        Refusing { ordered: true }
    }

    async fn disrupt(&self) {}
}

#[test]
fn the_ordered_cancel_scenario_is_skipped_on_a_link_declared_without_ordered_control() {
    OnTokio::block_on(ulo_rpc_conformance::cases::order::cancel_follows_its_request::<Unordered>());
}

#[test]
#[should_panic(expected = "the scenario started a server on the link")]
fn the_ordered_cancel_scenario_runs_on_a_link_declaring_nothing() {
    OnTokio::block_on(ulo_rpc_conformance::cases::order::cancel_follows_its_request::<Ordered>());
}
