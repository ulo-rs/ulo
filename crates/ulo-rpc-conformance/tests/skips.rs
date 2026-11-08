//! A scenario tied to a capability reads the link's declaration: on a link declared without
//! `ordered_control` the ordered-cancel scenario returns before it starts a link, and so does the
//! one-instance event scenario on a link declared without `single_delivery`; on a link declared
//! without `every_instance` the every-instance scenario asserts the server's refusal at startup,
//! which comes before any link starts. On a link declaring nothing each scenario runs. The link
//! here fails every call that starts it, so a run shows as a panic and a skip, or a refusal
//! asserted, as a return.

use std::future::Future;
use std::sync::Arc;

use ulo::{BoxError, Runtime};
use ulo_rpc::link::Inbound;
use ulo_rpc::{Capabilities, DeliveryMode, Link, Outbound, Subscription};
use ulo_rpc_conformance::{Broker, Harness, OnTokio};

/// Why a link declared without `ordered_control` lacks it.
const UNORDERED: &str = "the test link's `cancel` travels a lane of its own";

/// Why a link declared without `single_delivery` lacks it.
const FANNED: &str = "the test link hands each message to every instance";

/// Why a link declared without `every_instance` lacks it.
const ADDRESSED: &str = "the test link's caller names one server";

/// What a [`Refusing`] link declares absent.
#[derive(Clone, Copy)]
enum Without {
    Nothing,
    OrderedControl,
    SingleDelivery,
    EveryInstance,
}

/// A link that refuses to be started, declaring one capability absent or none.
struct Refusing(Without);

impl Link for Refusing {
    const NAME: &'static str = "refusing";

    fn capabilities(&self) -> Capabilities {
        let capabilities = Capabilities::new(DeliveryMode::Addressed);
        match self.0 {
            Without::Nothing => capabilities,
            Without::OrderedControl => capabilities.without_ordered_control(UNORDERED),
            Without::SingleDelivery => capabilities.without_single_delivery(FANNED),
            Without::EveryInstance => capabilities.without_every_instance(ADDRESSED),
        }
    }

    fn listen(&self, _: &[Subscription]) -> impl Future<Output = Result<Inbound, BoxError>> + Send {
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

/// The environment of a [`Refusing`] link declaring `W` absent.
struct Env<const W: u8>;

impl<const W: u8> Broker for Env<W> {
    type Harness = OnTokio;
    type Link = Refusing;

    async fn start() -> Self {
        Env
    }

    fn link(&self) -> Refusing {
        Refusing(match W {
            1 => Without::OrderedControl,
            2 => Without::SingleDelivery,
            3 => Without::EveryInstance,
            _ => Without::Nothing,
        })
    }

    async fn disrupt(&self) {}
}

type DeclaringNothing = Env<0>;
type Unordered = Env<1>;
type FannedOut = Env<2>;
type AddressedOnly = Env<3>;

#[test]
fn the_ordered_cancel_scenario_is_skipped_on_a_link_declared_without_ordered_control() {
    OnTokio::block_on(ulo_rpc_conformance::cases::order::cancel_follows_its_request::<Unordered>());
}

#[test]
#[should_panic(expected = "the scenario started a server on the link")]
fn the_ordered_cancel_scenario_runs_on_a_link_declaring_nothing() {
    OnTokio::block_on(ulo_rpc_conformance::cases::order::cancel_follows_its_request::<DeclaringNothing>());
}

#[test]
fn the_one_instance_event_scenario_is_skipped_on_a_link_declared_without_single_delivery() {
    OnTokio::block_on(ulo_rpc_conformance::cases::delivery::one_instance::<FannedOut>());
}

#[test]
#[should_panic(expected = "the scenario started a server on the link")]
fn the_one_instance_event_scenario_runs_on_a_link_declaring_nothing() {
    OnTokio::block_on(ulo_rpc_conformance::cases::delivery::one_instance::<DeclaringNothing>());
}

#[test]
fn the_every_instance_scenario_asserts_the_startup_refusal_on_a_link_declared_without_every_instance() {
    OnTokio::block_on(ulo_rpc_conformance::cases::delivery::every_instance::<AddressedOnly>());
}

#[test]
#[should_panic(expected = "the scenario started a server on the link")]
fn the_every_instance_scenario_runs_on_a_link_declaring_nothing() {
    OnTokio::block_on(ulo_rpc_conformance::cases::delivery::every_instance::<DeclaringNothing>());
}
