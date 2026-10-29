# 32 graphql subscriptions

Subscriptions over the `graphql-transport-ws` subprotocol. `GraphqlConfig::subscriptions(path)` mounts the gateway beside the endpoint, and the application imports `WsModule` once for it to be reachable. The `ticks` subscription reads from a `Ticker` provider; whatever calls `Ticker::publish` feeds every open subscription.

```
cargo run -p ex-32-graphql-subscriptions
```

The server listens on `PORT` (default 3000); the gateway is at `ws://127.0.0.1:3000/graphql/ws`. The binary has nothing that publishes, so the tests are where ticks flow.

Read first:

- `src/lib.rs`: the `Ticker`, the subscription, and the module wiring.
- `tests/subscriptions.rs`: ticks arrive in publish order; two operations on one connection each get every tick under their own id; the drain ends a running operation with `complete` and closes the socket with 1001 and `the server is shutting down`. The drain runs on a scripted clock (`ScriptedTimer`), as in example 10.
- `ex-support`'s `graphql` module: the handshake (`connection_init`, `connection_ack`) and the `subscribe` message.
- `tests/binary.rs`: runs the compiled binary, completes the handshake and ends it with SIGTERM.

This is the last example in the GraphQL part.
