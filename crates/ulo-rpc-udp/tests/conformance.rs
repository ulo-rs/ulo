//! The RPC conformance suite over the UDP link, each scenario against a server on a port of its
//! own. The link declares unary calls alone and a 65,507-byte frame, so the streamed scenarios and
//! the binary one assert the startup refusals, the three stream scenarios the client's
//! `shape_unsupported` refusal besides, and the oversized one the client's refusal one byte over
//! the datagram. The four scenarios sending a streamed request's `open` below the server, the
//! recovery scenario and the every-instance event's delivery are not applicable or refused as
//! declared below, and the ordered-`cancel` one is skipped from the link's declaration without
//! `ordered_control`.

use std::net::{Ipv4Addr, SocketAddr};

use ulo::BoundAddr;
use ulo_rpc::Link;
use ulo_rpc_conformance::{Broker, startup_failed};
use ulo_rpc_udp::Udp;

struct Loopback;

impl Broker for Loopback {
    type Harness = ulo_rpc_conformance::OnTokio;
    type Link = Udp;

    async fn start() -> Self {
        Loopback
    }

    /// Port 0: the OS chooses the server's port as it binds.
    fn link(&self) -> Udp {
        Udp::new(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
    }

    /// Aimed at the address the server bound.
    fn client_link(&self, server: &[BoundAddr]) -> Udp {
        let [bound] = server else { startup_failed!("expected one bound server address, got {server:?}") };
        Udp::new(bound.addr)
    }

    /// UDP holds no connection to sever, and the recovery scenario is declared not applicable.
    async fn disrupt(&self) {}
}

ulo_rpc_conformance::conformance_suite!(Loopback; not_applicable {
    recovery_after_disrupt: "UDP holds no connection to lose",
    cancel_before_opened: "UDP carries no streamed request, so there is no `open` to cancel; `client_stream`, `client_stream_in_order` and \
                           `bidi_stream` assert its startup refusal of a streamed handler",
    a_late_opened_releases_the_held_cancel: "UDP carries no streamed request, so no `cancel` is held for an `opened`; `client_stream`, \
                                             `client_stream_in_order` and `bidi_stream` assert its startup refusal of a streamed handler",
    a_held_cancel_is_dropped_when_its_hold_runs_out: "UDP carries no streamed request, so no `cancel` is held for an `opened`; `client_stream`, \
                                                      `client_stream_in_order` and `bidi_stream` assert its startup refusal of a streamed handler",
    a_held_cancel_lasts_until_its_call_s_deadline: "UDP carries no streamed request, so no `cancel` is held for an `opened`; `client_stream`, \
                                                    `client_stream_in_order` and `bidi_stream` assert its startup refusal of a streamed handler",
});

/// Datagrams on loopback arrive in the order they were sent, so the ordered-`cancel` scenario
/// would pass here on a link declaring `ordered_control`, though a network reorders them. The
/// declaration is what skips it, and this pins the declaration.
#[test]
fn the_link_is_declared_without_ordered_control() {
    let capabilities = Udp::new(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).capabilities();
    assert!(
        !capabilities.ordered_control() && capabilities.why_not_ordered_control().is_some_and(|reason| reason.contains("any order")),
        "{capabilities:?}"
    );
}
