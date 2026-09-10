# #96 — fix(mqtt): resubscribe on reconnect

Merged 2026-06-11 into `master` from `feat/mqtt-reconnect`, commit [`538eeab`](https://github.com/ulo-rs/ulo/commit/538eeabe7538144bac95973a1750ab452f75cfff).

A dropped broker connection left MQTT RPC silently dead until process restart: rumqttc reconnects the underlying socket but does not replay subscriptions, so after any blip the server's handler topics and the client's reply topic were no longer subscribed.

The fix drives `subscribe` off the `ConnAck` event, which fires on every connect — initial and reconnect alike — so subscriptions are re-established each time the socket comes up.

## Changes

- **Server** ([mqtt_adapter.rs](toni-mqtt/src/mqtt_adapter.rs)) — resubscribe every pattern on `ConnAck` instead of once before the poll loop.
- **Client** ([mqtt_client_transport.rs](toni-mqtt/src/mqtt_client_transport.rs)) — resubscribe the private reply topic on `ConnAck`; the existing SubAck-ready wait is unchanged.
- **Test** ([reconnect.rs](toni-mqtt/tests/reconnect.rs)) — pauses the broker (cgroup freezer) long enough to trip the keepalive, then unpauses, and asserts `send` recovers. Pausing keeps the published host port stable, which stop/start does not. Verified the test fails against the pre-fix subscribe-once behavior.
