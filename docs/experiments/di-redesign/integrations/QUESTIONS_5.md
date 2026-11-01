# Questions after the rulings and the core test series (for the designer)

Written for: the designer. Everything in the eighth and ninth responses is built, checked by a second model and on `experiment/di-redesign` at `036ba437`; CI is green across all 38 jobs (the 22-project template matrix, the container-backed store suites, the trybuild diagnostics job, MSRV, miri, smol, docs). The workspace runs 1924 tests with no failure. The core test series is complete: about 700 tests and 28 trybuild cases, each shown failing under a planted break, with an independent check after every batch that found and closed 51 weak tests. Four real defects surfaced and are fixed: F450 (`AfterInit` ran twice with two HTTP servers), F455 (a zero drain window never entered a server's drain), F456 (async-graphql answered a missing required variable as an executed result) and F457 (`Valid<Payload<T>>` did not compile on an RPC handler). Each item below needs a ruling, a confirmation or a choice the designs do not make. Each carries a recommendation. Detail: `CORE_TEST_PLAN.md` (status paragraphs per batch), `divergences/race3-step5.md`, `FRAMEWORK_GAPS.md` F449 to F457.

Paste alongside: core `DESIGN.md` §7, §9.5, §10.1, §12; `CORE_TEST_PLAN.md` §7.

## Rulings that changed how a built behaviour reads

**A1. F455 and Q13: a spent drain window now enters each server's drain once.** `drain()` dropped the drain futures unpolled when the window was zero (an expired `shutdown_timeout` or `drain_timeout(ZERO)`), so no transport stopped accepting. It now polls them once, as an expired cap gives a `close`. An app with no `Timer` (`DrainEnd::Now`) also enters each drain once. Confirm, in particular the no-timer case.

**A2. `BoundWithoutTimer` and a wait's reported location.** §10.1 step 6 says a wait is reported "at the call that wrote the wait". The code records the registering call (`.singleton`, `.ready`, `.on_init`) for a construction, readiness or hook bound, and only `.backoff` records its own line. The tests pin what the code does.
- Recommendation: record the `.timeout(..)` call, because that is the line a developer edits. If that costs more than it is worth, amend §10.1 to say "the call that registered the item that carries the wait".

**A3. Is there a zero rule?** Your twelfth ruling said a zero `roots_timeout` "is refused by the zero rule". No zero rule exists in the design or in `ulo`: `Bound::After(ZERO)` passes through `resolve_bound` in the core, and the NATS link refuses it on its own. Should the core refuse a zero bound for every item, naming the call? Recommendation: yes, one rule in `resolve_bound`; the NATS refusal then becomes redundant.

**A4. Controller-level `value = expr` (F453).** It compiles, builds one instance and shares it across handlers, as transports X2 says. Core §7 (line 664) and the §12 row (line 1265) still list it as refused. Recommendation: correct core §7 and §12 to X2. Built-once is pinned only for `guard_arc`; the macro form's sharing has no test. Add one in `ulo-codegen-tests`?

**A5. The spurious error beside the `FromContainer` refusal (F454).** A non-`FromContainer` field gets the refusal twice and a third, misleading error ("a `Auto` type cannot read `String` ... needs an execution") from the `AllowedIn` bound. It is not a one-line fix: `AllowedIn<S>: FromContainer` breaks `ulo`'s own build, because `Dep`'s `AllowedIn` impl is unbounded while its `FromContainer` impl needs `Send + Sync`. Recommendation: restructure so the bound sits in the macro's assertion, and the three-error output becomes one.

**A6. `HandlerInfo` set before the first guard.** Moving the set to after admission left all 304 core tests green; nothing reads it before the guards. Is it a guarantee that a guard can read the handler's info? Recommendation: yes, and pin it.

## Lifecycle semantics

**B1. A kept handle after the App drops (F451).** The phase advances to `Closed` at the owner's drop, as ruled. Probed with an axum host over `Embedded`: `GET /hi` answered 200 before `drop(app)` and 503 after, and 200 after when the advance was removed. A supervisor that keeps serving through a kept handle after dropping the App now gets 503s. Is that a supported shape?

**B2. F452: a `serve` result discarded after a trigger.** Once a trigger has been seen, any later `serve` `Err` in the same poll appears neither in the outcome (as ruled) nor in any log. Recommendation: a `tracing::warn!` there, five lines, outcome unchanged.

**B3. What an abandoned call sees is not designed.** Observed when the drain abandons a held call: HTTP gets an empty response with no status line, gRPC a broken pipe, WebSocket a reset with no closing handshake, RPC `Unavailable` ("the call was cancelled"; without cancellation the text differs, the code does not). Recommendation: gRPC `UNAVAILABLE` status, WebSocket close 1001, HTTP 503 where the response has not begun.

**B4. An expired shutdown cap with `Bound::Unbounded`.** `interval.rs` says the health task's own end covers a before-shutdown hook the cap skipped. With no interval there is no task, so `""` never turns `NOT_SERVING`. Recommendation: write `NOT_SERVING` from the drain itself.

**B5. `close_servers` is not the contract.** Removing it entirely stays green, because `release_servers` drops the servers anyway. Is `close` on a server a promise, or only the drop? Recommendation: a promise; a server whose `close` never ran is a test failure.

**B6. Panic in a handler's `prepare` inside `prepare_app`.** The hand-off's holder count never returns to zero, and its table keeps the app alive. `listen` catches no panics in `prepare`. Found by reading, not probed. Recommendation: catch and report as a `Configure` entry.

## Stores, config and transports (found checking the rulings batches)

**C1. SQLite lock wait.** The wait now ends about 10 ms before the bound; a lock freed in the last 10 ms is missed by design. Accept?

**C2. Nested `Keyed`.** `store.keyed::<A>().keyed::<B>()` prints `@ B` for the identity but exports and names the check `@ A`; two such modules with different outer keys fail wiring as an ambiguous `String @ A`. Recommendation: refuse the nested form at registration.

**C3. A short secret.** The scrub skips secrets under four characters, but the graph's redaction still replaces them in every error message, so a one-character secret mangles messages there.

**C4. Root certificates.** A root certificate given only by `PGSSLROOTCERT` is not read in `create`; a missing one stays retryable. A panicking SQLite lock thread (sqlx and diesel) reads as a retryable lock-step failure. webpki does not check a trust anchor's validity period, so an expired root still verifies a valid leaf (reasoned, not run).

**C5. `reqwest-blocking` OTLP.** Under that feature the environment's headers still win and a proxy choice is refused at `build`. Bring it in line with the ureq client, or leave the documented exception?

**C6. gRPC TLS failure.** `ulo-grpc/src/server.rs:330` pushes `error.to_string()`, erasing the `TlsError`, so a test cannot tell a certificate failure from any other configure failure. Recommendation: keep it typed.

**C7. A wrong-typed GraphQL variable.** async-graphql still answers an `Int` given a string as an executed result with a path, where juniper answers a request error. Closing it means full coercion against the declared types in the adapter. Recommendation: leave it filed, and say in the docs that the engines differ here.

**C8. Config problem order.** Source failures, then value problems, then missing fields; both reorder breaks stay green because the tests sort the problems. Is that order a promise?

**C9. `graphql-transport-ws`.** The close reasons for 4401, 4429, 4400 and 4406 are unpinned. After the server sends 4429, a running operation is not cancelled within 5 s while the client holds the socket. `subscribe` during the drain ("the server is shutting down") is untested. Recommendation: cancel on the server's own close, and pin the spec's reason strings.

**C10. Bad `tracestate` in pass-through.** Its text is still forwarded beside a valid `traceparent`. §6.3 says it "never affects `traceparent`" and is silent on whether it travels on. Recommendation: forward it as received.

**C11. Multipart overrun.** An over-limit multipart body with no `Content-Length` surfaces as a multer error from `next_field`; a handler propagating it with `?` answers 500 because the source chain is unclassified. Recommendation: classify it as 413.

**C12. A scoped pre-dispatch entry covering no route** is accepted silently; only an empty scope is refused. Recommendation: refuse at wiring.

**C13. HTTP chunked bodies.** The O1 harness sends bodies as one frame without `Content-Length`, the chunked path. With an 18-byte body and an 8-byte limit it answers 200 without the header and 413 with it. Percent-decoding, trailing slashes and SSE frame delivery are shown only against `AppService`; a host that buffers SSE frames would pass every HTTP test and the conformance suite. Should `ulo-http-conformance` cover them across hosts?

## The core test series

**D1. Capabilities with no home.** Capability 36 (per-module middleware by route) is not in `crates/ulo`; scoped `PreDispatch` with `exclude` replaced it and is tested in `ulo-http`. Drop 36 from `CAPABILITIES.md`, or reassign it? Capability 9 ("no string keys") is pinned in trybuild: does `DynamicModule`'s string id count as a key? The tests assume not.

**D2. Is a cycle through `Option<Dep<_>>` or `Many<_>` a cycle?** The series tests only plain cycles.

**D3. Four gaps nobody pins.** A binding hook resolving its parameters with root visibility instead of its module's; a keyed module's hook failure losing its qualifier; a nested construction failure naming the wrong module; `ExecutionRequired` at an `Ext` or `ExecutionRef` site. Pin them, or leave?

**D4. Reading of two errors.** `OverrideKind` reports `expected` as the kind the key is bound as now and `found` as the kind the override would make. `in_module_of(&config)` has no lookup (`module_of(&config)`); the tests reach a configured instance through `load(&config)`. Confirm the first; add the second?

**D5. Gateways in the container (capability 24).** A singleton gateway with a `Dep` exists in `ulo-ws-hyper`, but no test pins it as a singleton, and no gateway built per execution exists. Is per-execution a capability?

**D6. Controller injection.** `m.controller::<C>()` is an ordinary binding and a provider in the same module reading `Dep<C>` wires. The inventory listed "a controller cannot be injected" as a master property. Intended?

**D7. A lazy store's check** pings on every run and keeps no state, so "stays down until its first ping" is not a property; the store conformance suite owns outage and recovery. Confirm.

## For the record (no ruling needed)

- The sqlx lock-removal break stays green on sqlx and the ordering break too; sqlx's own migrator lock covers both. Diesel and SeaORM on Postgres prove ulo's lock.
- Two premises did not hold when built: a TOML, YAML or JSON parse error quoting a secret (none of the shipped parsers quotes file content; the scrub is tested through a crate-internal failing source), and Redis retries at 3 being better than 0 (recovery time is identical).
- The gRPC admission bounds answer `UNAVAILABLE`, not `RESOURCE_EXHAUSTED`; the inventory's old name was wrong.
- `ModuleDef::qualifier()` returns `Option<TypeName>`, so the colliding rule has a type id.
- `UpgradeHandler` gained no method; the holder count lives inside `Handoff`.
- A reusable lesson from the checks: a test that asserts the variant but not which module, key, route or header it names stays green under a break that names the wrong one. All 51 closed defects were of that class.
