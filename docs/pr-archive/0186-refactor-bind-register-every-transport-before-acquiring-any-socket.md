# #186 — refactor(bind): register every transport before acquiring any socket

Merged 2026-08-24 into `master` from `refactor/bind-registers-before-acquiring`, commit [`0b36f33`](https://github.com/ulo-rs/ulo/commit/0b36f3309a0a127c843edfff3b4d19fe34c98cd6).

`bind` ran registration and acquisition per transport: take the WebSocket gateways, open their sockets, then ask the RPC adapter for its patterns, then open its socket, and so on. ADR-0009 separates these at the SPI level — `register_*` registers, `into_lifecycle` takes the socket — but `bind` interleaved them.

So a declaration that cannot work, such as a route on an adapter with no support for it, failed with sockets already listening and went through the teardown path that exists for the case where the OS refuses.

Every adapter is now asked to take what the application declares before any of them is asked for a socket. Three consequences:

- a programmer error fails with nothing acquired
- teardown is reachable only by socket failures, which is a narrower invariant to keep true
- the order failures surface in stops depending on which transport came first — an application that is wrong reports that before an environment that is busy

Every ordering constraint is preserved: each `register_*` still precedes its own adapter's `into_lifecycle`, which is where the native router is built. `gateway.call_after_init()` already fired between registration and acquisition and does not move.

### The error variants are unchanged

ADR-0024 defines `Adapter` as covering both "failed to take its handlers" and "failed to acquire its socket", and it names the transport that refused either way. Splitting it along the new phase boundary would redefine a documented variant for no gain.

### Verification

`a_refused_registration_fails_before_any_socket_is_taken` declares a separate-port WebSocket gateway and an RPC adapter that refuses its patterns. The refusing adapter tries to bind the gateway's port at the moment it is asked, so the test observes whether the socket existed *during* the failure rather than after it — which is the only externally visible difference, since teardown frees the socket either way.

The test fails against the interleaved order, where the gateway's port reads as taken, and passes here, where it reads as free.
