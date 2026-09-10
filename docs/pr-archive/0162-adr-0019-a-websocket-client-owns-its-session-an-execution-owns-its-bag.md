# #162 — ADR 0019 — a WebSocket client owns its session; an execution owns its bag

Merged 2026-08-21 into `master` from `docs/adr-client-owns-its-session`, commit [`c2bda42`](https://github.com/ulo-rs/ulo/commit/c2bda427f274ef0a120282dc2ccd2f3f405260fb).

Revises one decision in ADR 0018 (#160): the session moves onto `WsClient`, and `WsClient::extensions` is removed.

The rest of 0018 stands — store rather than context, a connection's lifetime, `Drop` as teardown, disconnect as an execution, no injectable and no session provider scope.

**The mismatch.** `WsClient` exists once per connection and its `extensions` field is the *execution's* bag, repointed on every context construction. That is not incidental: the repointing line is what produced three bags across one connect. It repoints a clone, the connection stored the original, and a third context was built for the hook.

**Why 0018's reasoning no longer holds.** It declined to put the session on `WsClient` because it would sit beside `extensions` and the two would be told apart by name alone. `WsClient::extensions` has one reader in the repository, a test, and one writer, the repointing line — a handler that wants the execution's bag takes `Extensions` as a parameter, the shape ADR 0015 established. Removing the first field is what makes room for the second.

The decision:

- `WsClient::new` creates the session, so a connection and the state scoped to it come into being together.
- `WsClient::extensions` is removed. The execution's bag has one owner, read as `ctx.extensions()` or taken as a parameter, on WebSocket exactly as on the other transports.
- `WsContext::new` mutates nothing it is given, and no clone of a client can disagree with another about which bag is current.
- Reached through an accessor rather than a public field. `client.session = Session::new()` on a clone is the shape of the defect being unwound, and a private field makes it unrepresentable.

0018 keeps a pointer to this on the section it revises. Docs only; the implementation follows separately.
