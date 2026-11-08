# ulo-rpc-conformance

The conformance suite every `ulo-rpc` link runs. A link is the transport under `ulo-rpc`'s client
and server: TCP, UDP, a broker, or one written outside the `ulo` repository. One macro stamps the
suite's scenarios as `#[test]`s against the link: requests and replies, events, the four call
shapes, cancellation, deadlines, drain, recovery from a dropped connection, two instances on one
pattern and admission under load. A link whose `tests/conformance.rs` passes keeps the same
promises as the links `ulo` ships, each of which runs the suite in CI.

## Install

The suite is a dev-dependency of the link crate. The example below runs it against `ulo-rpc-tcp`,
which stands in for your link:

```toml
[dev-dependencies]
ulo = "=0.1.0-alpha.2"
ulo-rpc-conformance = { version = "=0.1.0-alpha.2", features = ["tokio"] }
ulo-rpc-tcp = "=0.1.0-alpha.2"
```

Each crate is pinned with `=`, since a caret on a pre-release also takes a later alpha, which may
break this one. `tokio` brings `OnTokio`, the runtime half of a link built on tokio; a link on smol
enables `smol` and names `OnSmol` instead.

## First run

`tests/conformance.rs`:

```rust
use std::net::{Ipv4Addr, SocketAddr};

use ulo::BoundAddr;
use ulo_rpc_conformance::relay::Relay;
use ulo_rpc_conformance::{Broker, OnTokio, startup_failed};
use ulo_rpc_tcp::Tcp;

/// One scenario's environment: a server on a port of its own, the client reaching it through a
/// relay that `disrupt` cuts.
struct Loopback {
    relay: Relay,
}

impl Broker for Loopback {
    type Harness = OnTokio;
    type Link = Tcp;

    async fn start() -> Self {
        Loopback { relay: Relay::without_upstream().await }
    }

    /// The server's link, on port 0: the OS chooses the port as the server binds.
    fn link(&self) -> Tcp {
        Tcp::new(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
    }

    /// The client's link, through the relay, aimed at the address the server bound.
    fn client_link(&self, server: &[BoundAddr]) -> Tcp {
        let [bound] = server else { startup_failed!("expected one bound server address, got {server:?}") };
        self.relay.forward_to(bound.addr);
        Tcp::new(self.relay.addr())
    }

    async fn disrupt(&self) {
        self.relay.cut().await;
    }

    async fn client_connections(&self) -> Option<usize> {
        Some(self.relay.open().await)
    }
}

ulo_rpc_conformance::conformance_suite!(Loopback; not_applicable {
    a_late_opened_releases_the_held_cancel: "a request and its control frames share one ordered lane, so the link holds no `cancel` for an `opened`",
    a_held_cancel_is_dropped_when_its_hold_runs_out: "a request and its control frames share one ordered lane, so the link holds no `cancel` for an `opened`",
    a_held_cancel_lasts_until_its_call_s_deadline: "a request and its control frames share one ordered lane, so the link holds no `cancel` for an `opened`",
});
```

`cargo test --test conformance` runs every scenario as a test of its own, named after the scenario,
and reports the three declared above as ignored with their reasons.

Your link takes `Tcp`'s place. `start` answers an environment no other scenario shares: a fresh
broker or namespace, or nothing where the server binds port 0. `link` answers the server's link,
`client_link` the client's once the servers are bound, and `disrupt` severs the client's
connection. A link run against a broker in a container starts it in `start`, through
`relay::unshadowed` when the container's port is published on the host, and fails a start that
does not come up through `startup_failed!`. The suite depends on no container library; the link's
own dev-dependencies bring one.

[`community-link`](https://github.com/ulo-rs/ulo/tree/master/crates/ulo-codegen-tests/fixtures/community-link)
is a complete link written as a crate outside the workspace would write it, against `ulo-rpc`'s
public link API alone, with its `tests/conformance.rs` run in CI.

## What a link declares

A link's `Capabilities` hold every ability unless the link declares one absent with
`without_<ability>(reason)`. A link that declares nothing is held to every scenario, and fails the
ones it cannot keep. A declared absence changes what the scenarios assert: a scenario whose ability
is absent asserts the refusal the declaration names, or, where it cannot run without the ability,
is skipped with one line naming the scenario, the ability and the reason on standard error.

`not_applicable { scenario: "reason" }` is for a scenario no capability describes, such as the
held-`cancel` scenarios above on a link that holds no `cancel`. The test is stamped
`#[ignore = "<reason>"]`, a name that is no scenario fails to compile, and
`cargo test -- --ignored` runs it, where it fails. A link that does hold a `cancel` until the
server's `opened` declares the same three not applicable too: they read the link's client table
through a probe outside the suite's public API, which the links `ulo` ships attach.

## Scenarios

The scenarios are the functions of the [`cases`](https://docs.rs/ulo-rpc-conformance/latest/ulo_rpc_conformance/cases/)
modules, each documented with what it asserts.

## Will it work for me

- **Rust 1.88** or later.
- **Runtimes:** the suite names none. `tokio` and `smol` bring the two harnesses; another runtime
  implements `Harness`.
- **Parallelism:** the scenarios run in parallel under libtest. `Broker::PARALLEL` bounds how many
  hold an environment at once, and a host short of memory sets `ULO_CONFORMANCE_PARALLEL` to a
  positive integer; the smaller of the two applies.
- **Startup failures** are written to a file before the test panics: under `$ULO_CONFORMANCE_FAILURES`
  when it is set, otherwise under `conformance-failures` in the target directory the test binary
  was built into.
- **Stability:** the scenarios, `Broker`, `Harness` and `conformance_suite!` are the contract a link
  is held to. While the version is a pre-release, every `ulo` crate releases at one version and a
  pre-release may change any of them. From `0.1.0` on, a release that adds or tightens a scenario
  or changes the contract is a minor release, `0.1` to `0.2`, since a link that passed the earlier
  suite can fail it.

## Where to go next

- The API docs on [docs.rs](https://docs.rs/ulo-rpc-conformance), or `cargo doc --open`.
- The suites of the links `ulo` ships: [`ulo-rpc-tcp`](https://github.com/ulo-rs/ulo/blob/master/crates/ulo-rpc-tcp/tests/conformance.rs)
  and [`ulo-rpc-udp`](https://github.com/ulo-rs/ulo/blob/master/crates/ulo-rpc-udp/tests/conformance.rs),
  run on every build, and the brokers' behind each crate's `integration` feature, a container per
  scenario:
  [NATS](https://github.com/ulo-rs/ulo/blob/master/crates/ulo-rpc-nats/tests/conformance.rs),
  [Redis](https://github.com/ulo-rs/ulo/blob/master/crates/ulo-rpc-redis/tests/conformance.rs),
  [MQTT](https://github.com/ulo-rs/ulo/blob/master/crates/ulo-rpc-mqtt/tests/conformance.rs),
  [RabbitMQ](https://github.com/ulo-rs/ulo/blob/master/crates/ulo-rpc-rabbitmq/tests/conformance.rs)
  and [Kafka](https://github.com/ulo-rs/ulo/blob/master/crates/ulo-rpc-kafka/tests/conformance.rs).

## License

MIT.
