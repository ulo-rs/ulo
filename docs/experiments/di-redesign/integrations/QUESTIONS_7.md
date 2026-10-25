# Questions after the eleventh response is built and checked (for the designer)

Written for: the designer. Every ruling in the eleventh response (E1 to E15) is built, checked by a second model and on `experiment/di-redesign` at `75c91d87`. CI is green. The workspace runs 2091 tests with no failure. The checks found 4 behaviour defects, all closed: a deadlock when a handler awaits its app's shutdown inline, a spent cap that no longer bounded `close`, a `close` that could outlive the cap by a first-poll delay, and `m.value(Secret<_>)` registering only `Secret<String>`. Each item below needs a ruling or a confirmation. Each carries a recommendation. Detail: `CORE_TEST_PLAN.md` (the eleventh-response status sections), `FRAMEWORK_GAPS.md`.

Paste alongside: core `DESIGN.md` §3.7, §3.9, §9.3, §9.5; transports `DESIGN.md` §2.6, decision 40.

## A ruling the build could not keep as written

**G1. E3 is not written by `close` itself.** Hyper owns the sockets, so `close` cannot write the abandon rendering. Built instead: a new `ulo_transport::Turns` lets a server's `close` poll each of its tasks once, on its first poll, newest task first, so a call's task runs before the connection that writes its answer. HTTP and gRPC go through `Serve::close_after`, WebSocket through `Tracker::close`, RPC through `Server::close`. Under `shutdown_timeout(ZERO)` the clients read the HTTP 503 document, the WebSocket 1001 close, and RPC and gRPC `Unavailable` with the shutdown phrase, 20 of 20 on a single-threaded runtime and 40 of 40 on four workers. On a multi-threaded runtime a `close` whose connections have already ended can finish in its first poll, so it is absent from the list of cut closes. Confirm this reading of E3, or name a different mechanism.

**G2. E4's gRPC half did not reproduce.** A gRPC client that stops reading, with its request body ended or left open, did not hold the server's `close` past the drain window. The concurrent-close test uses two WebSocket servers with frozen clients: about 0.7 s, against 1.2 s with the closes run one after another. Confirm, or name the gRPC scenario that holds a `close`.

## Behaviour the build added

**G3. A task poll already under way gets one more poll after a cancellation.** The check found that `Turns` deadlocked when a handler awaited `handle.close(..)` inline, and that a blocked poll made `close` outlast a spent cap. The fix takes the slot lock with `try_lock`; on contention it sets a flag and the running poll repeats once after a `Pending`, capped at one extra poll. `give` never waits. Confirm the cap of one, which keeps a future that calls `give` on every poll from looping.

**G4. RPC ends an always-ready stream at the next wait once a cancellation has been observed.** E2's pipeline-first order would otherwise run a stream whose items are always ready forever. Confirm the rule.

**G5. The WebSocket stream pump still checks cancellation before each item.** RPC lets an item ready in the same poll go out first. E2 says "pipeline first, everywhere" and names HTTP and RPC. Recommendation: change the WebSocket pump to the same order, one rule on four transports.

## Wiring the core's waits through `timeout` (E12)

**G6. Two waits went through `timeout`; one hand-written select remains.** The drain window and `bounded_close` now use `timeout()`. `lifecycle/run.rs::run` still hand-writes a select over two sleeps (the item's own bound and the cap); it can be written `timeout(own, timeout(cap, item))` with the same poll order. `cancel_at` keeps polling past its sleep and is not timeout-shaped. Does §3.7 count `run` as a copy? Recommendation: yes, rewrite it.

**G7. `m.value(Secret<_>)` registers `Secret<String>` and `Secret<&'static str>`, not every `Secret<T>`.** §9.3 says the call "registers the value it binds". A generic `Secret<T>` cannot be recognised from `value<T>` without specialisation. The check showed a `Secret<&'static str>` and a `Secret<CustomDisplay>` staying visible in a startup report. Built: the two types are registered, §9.3 now names them and sends any other `T` to `m.secret(&s)`. Confirm the narrowing, or rule a different mechanism (for example `value` requiring `T: Display`-independent registration through a sealed trait).

## HTTP and the stage

**G8. The older `Stage::build` refusals carry text only.** A stray `exclude`, an empty scope and an unparseable scope still answer with message text and no `location` or `subject`; "names no route pattern" has a location available. E7 made the fields optional, so this passes. Recommendation: add `.at(location).about("pre-dispatch entry")` to each, and move their tests off text matching.

**G9. gRPC had no private copy of the stage.** E8 expected one. `ulo-grpc` already built through `ulo_http::stage`; the missing piece was that `unreached` was `pub(crate)` and gRPC never called it. It is `pub` now and gRPC calls it after building its routes, skipped when the route build itself failed (HTTP does the same). Nothing to rule; recorded because the ruling's premise was a copy.

## Reach of the drain work

**G10. E3's first-poll rendering is verified on RPC over TCP only.** UDP and the five broker links are not run (their conformance suites need brokers). Embedded HTTP hosts (axum, salvo and the rest) get E1 and E2 through the shared service, but E3's turn reaches only hyper's own connections. Confirm that is the contract for an embedded host, which owns its sockets.

**G11. gRPC health under a spent cap is not re-probed.** The earlier note in `CORE_TEST_PLAN.md` (B4) says a health client cannot read `NOT_SERVING` over the wire under a spent cap. Whether the first-poll rendering changes that is unprobed. Recommendation: probe it, and file the result.

## Errors and macros

**G12. `TlsError::Rustls` is pinned to rustls's text.** The test for a valid base64 string that is not a key matches `rustls::Error::General("failed to parse private key as RSA, ECDSA, or EdDSA")`, which changes with the rustls version. `InconsistentKeys` maps only to `KeyMismatch` in practice, because rustls 0.23 swallows `Unknown` in `with_single_cert`. Recommendation: add a `KeyCause::Unrecognised` for that case, so the test matches a variant of ours.

**G13. The `construct` probe name has no emitter.** The F471 rule names `check`, `matches` and `construct`. No macro emits a method of that name by method syntax; the refusal that rests on `construct` is a supertrait bound. The name is pinned only as an entry in the blanket-trait list. Nothing to build; recorded so the rule's wording matches what exists.

## Interfaces the build named

**G14. `OtlpExporter::http_over(Client)` names the HTTP client.** E14 said "the constructor names the client". Built: `Client` is a `#[non_exhaustive]` enum with cfg-gated variants, `http()` uses `ureq` when that feature is on, `http_over(Client)` takes one, `client()` reads it. With neither feature a `compile_error!` fires, followed by a few secondary E0599 errors from `Client::default`. Confirm the names, and whether the secondary errors matter.

**G15. `CAPABILITIES.md` entry 36 stays as "Dropped. The number is not reused".** Removing it would shift the numbering that transports §6.1 and §6.3 cite. The §7 middleware paragraph now points to scoped `PreDispatch` with `exclude` and carries no `[36]`.

## Test gaps nobody pins

**G16. Four timing paths have no test that goes red when they are removed.** RPC refusal and unhandled-pattern tasks left untracked; the raw WebSocket inbox left untracked; gRPC's `CLOSE_GRACE` and hyper's `FLUSH` waits (no test has a client outlive them); `AbortOnDrop` not releasing listeners that never served. The first three are hard to time deterministically. Pin them with scripted clocks where the wait is a `Timer`, or accept the gap?

## For the record

- **E13 stands as ruled:** a missing MySQL client certificate is `Permanent` over TCP at the first ping, `Retryable` over a socket, and `create` succeeds. Not exercised against a MySQL container this round.
- **A CI flake closed:** a `ulo-rpc-nats` unit test read the roots directory at once after a drop; the removal is asynchronous. It waits for the directory to empty now.
- **The 200-run drain test** cannot go red by design. With the second drain check, 0 to 2 clients read nothing per 200 runs; with it removed, 4 to 11. Only the `drain.rs` unit test fails on that break.
- **The checks' tally:** 34 planted breaks against the drain and store builds, 9 survived, 6 closed by new tests and one test restructured; batch three's checker found 2 defects and 1 test gap.
