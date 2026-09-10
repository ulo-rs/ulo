# #44 — fix(ws): recover from handler panics and restore HTTP-scoped enhancer validation

Merged 2026-04-18 into `master` from `fix/ws-handler-panic-recovery`, commit [`1af58dc`](https://github.com/ulo-rs/ulo/commit/1af58dc505f4e06fdbcc6d63a36c7808fd4f99b0).

## What was broken

Two separate but related issues, both causing WS connections to hang indefinitely:

1. **Any panic in a WS message handler** killed the adapter's read-loop task silently.
   The disconnect callback never fired, leaving the client registered in the map
   forever with no signal sent to the client.

2. **`c7e5a10` removed the startup check** that prevented HTTP-injecting guards
   (e.g. a request-scoped guard with `#[inject] request: Request`) from being used
   on a WS gateway. RPC controllers kept the equivalent check; WS gateways silently
   lost it. Without the check, the guard resolved fine at startup, then panicked at
   message time with `HTTP request context required for request-scoped dependency`
   — triggering issue (1).

## Why the check was removed

The belief was that the check would block legitimate use of the HTTP upgrade
request inside `on_connect` handlers — e.g. reading handshake headers to auth
a connection. That concern was unfounded: the two paths are completely
independent.

- **Direct access in `on_connect`** — reading `ws.client().handshake.headers`
  is plain field access on what the framework already provides. The startup check
  never touched this path and restoring it does not affect it.
- **DI injection of `Request`** — declaring `#[inject] request: Request` in a
  request-scoped provider wires a dependency through the DI container. This is
  what the check guards against, because WS message handlers have no HTTP request
  context to satisfy that dependency at runtime.

Users can and should read upgrade request parts directly in `on_connect`. They
should not inject `Request` as a DI dependency into a provider used on a WS gateway.

## How it was investigated

Three states were tested with the same guard (injecting `Request` via DI on a WS gateway):

| State | Behavior |
|---|---|
| `c7e5a10` — no startup check, no catch_unwind | Panic swallowed silently → **hangs 60+ seconds** |
| Test commit only — no catch_unwind | Connection doesn't close within 500 ms → **test FAILS** |
| catch_unwind only, no startup check | Panic caught, connection closes → **FAILS in 2 s**, no hang |
| This branch (both fixes) | Startup check fires → **immediate exit with clear error** |

## Commits

1. **Test** — regression guard: connection must close within 500 ms of a handler panic; sibling connections must be unaffected. Fails without the fix.
2. **catch_unwind fix** — both adapters wrap the read loop in `AssertUnwindSafe + catch_unwind`; on panic the error is logged and the normal disconnect path fires.
3. **Startup check restored** — `gateway_resolver.rs` re-applies `requires_http_parts()` validation for guards, interceptors, and pipes (including handler-level), matching the RPC controller resolver exactly.

## Test plan

- [ ] `cargo test -p integration-tests --test integration -- --no-capture` — 129 tests pass
- [ ] Checkout `08b51a5` (test only) → `ws_panic_recovery` test **fails** — proves the test captures the bug
- [ ] Checkout `6005a61` (catch_unwind added) → test **passes** — fix confirmed
