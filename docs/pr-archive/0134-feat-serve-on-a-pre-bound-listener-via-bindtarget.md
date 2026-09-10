# #134 — feat!: serve on a pre-bound listener via BindTarget

Merged 2026-07-30 into `master` from `feat/bind-target`, commit [`2a33cca`](https://github.com/ulo-rs/ulo/commit/2a33cca546d0179ce9af812ba1696f09df183df9).

An HTTP adapter can serve on a socket the caller already bound. `BindTarget` carries either an address or a `std::net::TcpListener` that is listening, and replaces the port and hostname pair on `use_http_adapter` and on `HttpAdapter::into_lifecycle`. Both forms arrive through `impl Into<BindTarget>`, so `("127.0.0.1", 3000)` and a listener are equally direct.

This serves three callers that could not be served from outside the framework: a watch-restart development loop where the port keeps answering across a rebuild, systemd socket activation and zero-downtime restarts, and test harnesses that bind port 0 themselves rather than racing for it. Acquisition stays outside — core accepts a listener and asks nothing about its origin, so `listenfd` with systemfd or systemd supplies it and any supervisor speaking that protocol works.

**Core** — `BindTarget` (`#[non_exhaustive]`, with `From<(&str, u16)>`, `From<(String, u16)>`, `From<TcpListener>`, and `into_std_listener`), threaded from `use_http_adapter` through `bind()` into the HTTP SPI. `BoundAdapters` derives `Debug`.

**Adapters** — axum, poem, salvo, and actix adopt a supplied listener through their own listener entry points. Rocket cannot: it binds inside `launch()` from figment configuration with no hook for an existing listener, so it refuses that form with an error naming the limitation, the same capability-honesty pattern as `register_ws_route`'s default.

**Tests** — per-adapter adoption is proven by address identity: the suite records a listener's address before handing it over and requires the application to report and answer on that exact address, which a fresh bind on port 0 would not match. A second test covers what the feature is actually for — a client connecting while nothing is accepting is answered once the next generation starts, which holds only while the socket stays open across the gap. Rocket is pinned to refuse.

**Docs** — ADR-0013 records why the seam sits on the HTTP SPI alone: addressing is owned by core for HTTP, by the adapter for RPC and gRPC, and by the gateway declaration for separate-port WebSocket.

Breaking: `use_http_adapter(adapter, port, hostname)` becomes `use_http_adapter(adapter, (hostname, port))`, and `HttpAdapter::into_lifecycle` takes a `BindTarget`. Every implementation and call site is in-tree and migrated.

Deferred: `from_listener` constructors for the RPC and gRPC adapters, which own their addresses in their own constructors and need no SPI change; and separate-port WebSocket, where an inherited socket carries no indication of which gateway's declared port it satisfies.
