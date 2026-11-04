//! The RPC conformance suite over the in-process link, each scenario on a hub of its own.

use community_link::{Hub, Loopback};
use ulo_rpc_conformance::Broker;

struct InProcess {
    hub: Hub,
}

impl Broker for InProcess {
    type Harness = ulo_rpc_conformance::OnTokio;
    type Link = Loopback;

    async fn start() -> Self {
        InProcess { hub: Hub::new() }
    }

    fn link(&self) -> Loopback {
        Loopback::on(&self.hub)
    }

    async fn disrupt(&self) {
        self.hub.disrupt();
    }

    async fn client_connections(&self) -> Option<usize> {
        Some(self.hub.client_connections())
    }
}

ulo_rpc_conformance::conformance_suite!(InProcess; not_applicable {
    a_late_opened_releases_the_held_cancel: "a request and its control frames share one ordered queue, so the link holds no `cancel` for an `opened`",
    a_held_cancel_is_dropped_when_its_hold_runs_out: "a request and its control frames share one ordered queue, so the link holds no `cancel` for an `opened`",
    a_held_cancel_lasts_until_its_call_s_deadline: "a request and its control frames share one ordered queue, so the link holds no `cancel` for an `opened`",
});
