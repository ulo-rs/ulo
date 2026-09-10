# #159 — Pin what request scope means on a WebSocket

Merged 2026-08-20 into `master` from `test/ws-request-scope-is-per-message`, commit [`069b8c7`](https://github.com/ulo-rs/ulo/commit/069b8c7baa568ce8f93b9807c40e0d10dbc9efa9).

A request-scoped provider on a WebSocket is built once per message. Nothing covered that.

The answer is not the one Nest reaches. Its websocket controller hands the client object to the same `getContextId` the HTTP path uses, and an id stamped on the client survives the connection — so a request-scoped provider there is built once per connection. toni keys on the execution, and a message is an execution.

The test sends two messages on one connection and resolves two distinct instances. The provider is reached through a request-scoped guard, that being the only holder a gateway permits: a gateway is a singleton serving every connection on its path, and is refused a request-scoped dependency at startup.
