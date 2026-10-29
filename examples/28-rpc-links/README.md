# 28 rpc links

The handlers of [25](../25-rpc-basics), served over TCP, NATS, Redis, MQTT, RabbitMQ or Kafka. The link is a URL: `bind` and `client` pick the link crate by its scheme, and nothing else in the app changes.

```
cargo test -p ex-28-rpc-links
RPC_LINK=nats://127.0.0.1:4222 cargo run -p ex-28-rpc-links
```

`RPC_LINK` takes `tcp://`, `nats://`, `redis://`, `mqtt://`, `amqp://` or `kafka://`; without it the handlers listen on TCP at `PORT`.

The broker tests start each broker in a container named `ulo-conf-ex28-<link>` and need Docker:

```
cargo test -p ex-28-rpc-links --features integration --test brokers
```

Read first:

- `src/lib.rs`: `bind` and `client`.
- `tests/common/mod.rs`: what every link answers: a reply, a handler error with its kind and message, an event.
- `tests/brokers.rs`: one container per link.

What the links do not answer alike, an unhandled pattern among it, is pinned by the `ulo-rpc-conformance` suite each link crate runs.

Next: [29 grpc-service](../29-grpc-service).
