# #221 — Raise a typed event when nothing handles a call

Merged 2026-08-31 into `master` from `feat/unrouted-is-an-event`, commit [`d90079e`](https://github.com/ulo-rs/ulo/commit/d90079eca63c362e68e86e0f538e6f8088b32995).

An RPC pattern no controller claims was refused at the dispatcher with no context built, so no `ErrorObserver` fanned and no error handler was consulted — not even one registered through `use_global_rpc_error_handler`. Operationally that is the signal most worth having: a caller and a server disagreeing about what exists. It was the one call nothing could see.

Underneath it, the condition had no type. `GuardRejection`, `MiddlewareFailure` and `PanicRecovered` are typed framework events; "nothing handles this" was spelled four ways, so a handler could only match it by transport.

## `Unrouted`

Carries the target as the caller named it, `kind()` is `NotFound`. Raised in three places:

- the RPC dispatcher, which now builds a context for the miss, fans the event to the observers and offers it to the global chain before falling back
- the `#[patterns]`-generated dispatch, for a pattern the controller registered with nothing behind it
- the `#[subscriptions]`-generated dispatch, for an event no handler subscribes to

So `#[catch(Unrouted)]` covers every route to the condition on both transports.

## What a caller sees

Unclaimed, the frame keeps its shape: the RPC dispatcher still answers a `not_found` wire error, and WebSocket still answers `{"status":"error","kind":"NotFound",…}`. One string does change — the message reads `nothing handles X` where it read `Unknown event: X` or `Unknown pattern: X`, which were already two different texts for one condition.

## Not covered

HTTP and gRPC answer inside routing the framework does not own — the adapter writes the 404 (`toni-axum/src/axum_adapter.rs:440` and its four siblings), and tonic writes `Unimplemented`. Reporting those means giving the adapter SPI a way to say "nothing matched" rather than write the response, which is a larger change than this one.

## Tests

Five in `unrouted_is_an_event.rs`: the RPC miss reaches an observer, a handler can claim it on each transport, and the unclaimed rendering on each is what it was.
