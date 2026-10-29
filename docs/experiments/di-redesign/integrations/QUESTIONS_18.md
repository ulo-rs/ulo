# Questions after the example catalogue (for the designer)

Written for: the designer. The catalogue is built, checked by a second model per part and committed: examples 01 to 39, one package each under `examples/`, 2,493 tests in the workspace gate, 0 failed. Writing it was the first use of the API as a user will use it, so the findings below are the usability results. They fall in four groups: behaviour defects (P1 to P7), the S-items that want a ruling (P8 to P11), what the inventory cross-check left open (P12 to P14), and the CI flakes (P15). `QUESTIONS_17.md` (P1 to P4 there) is still open and is not repeated here. Detail: `FRAMEWORK_GAPS.md` F499 to F503 and the S-item entries under each part.

Paste alongside: core `DESIGN.md` §5 (the `AuditContext` example), §9.5; `SWITCH_INVENTORY.md` §1 (G1 to G26); `docs/experiments/di-redesign/integrations/QUESTIONS_17.md`.

## Behaviour defects

**P1. An execution-scoped service holding `ExecutionRef` leaks its execution (S11, HIGH, reproduced).** DESIGN §5's own `AuditContext` example is a reference cycle: the execution never ends. Standalone, `close` waits the whole drain window and reports `abandoned = 1` for a call that had finished; over HTTP, three finished GETs report `abandoned = 3` after 10 s, and every request leaks its whole execution. Contradicts §5 and §3.8. Options: (i) cached instances hold the execution weakly; (ii) the cache and live slot are released when the last external handle drops. Recommendation: (i). The cycle is the cache holding the thing that owns the cache, and a weak edge removes it without changing when instances are dropped. Example 12 avoids the pattern and says so.

**P2. `HttpHandler` is only in `ulo_http::__private`, so "macros are sugar" does not hold for controllers (S16).** First-party `ulo-health-http` and `ulo-graphql-http` import it from there. Example 14 shows macro-free providers and modules but has no controller half. Options: (i) a public `ulo_http::HttpHandler::new` with the signature the first-party crates already use; (ii) say in DESIGN §0.2 and §13 that controllers need the macro. Recommendation: (i). Two crates already depend on the path.

**P3. An undeclared dependency in a hand-written `Construct` wires and runs (S17).** DESIGN §0.1 promises wiring catches it. Suggested: a debug check of reads against declared keys. Recommendation: do it in debug builds only.

**P4. `ulo-ws-redis` `subscribe()` returns before Redis confirms the SUBSCRIBE (F499, read).** `listen` finishes before the subscription is live, and all delivery, same-process members included, goes through it, so a fresh process drops broadcasts it should deliver. Fix: await readiness after the subscribe replies. Recommendation: do it. It fails silently today.

**P5. The config pass ends at the first leaf error and invents a second problem (F501, medium, reproduced).** A leaf type's own `Deserialize` error ends the pass for the whole tree; a refused `database.url` beside `http.port = "eighty"` reports only the first. A missing `url` also yields a spurious "invalid value, found nothing" with origin `Default` beside "missing required field", and that spurious refusal ends the pass, so `http.port` is never reported. "Reports every problem" is §3.1's promise for wiring and the config docs make the same one. Recommendation: do not feed a placeholder to a leaf type's `Deserialize`, and read top-level sections separately.

**P6. `.lazy().migrate(..)` wires and then fails at connect (F502).** The migration lock is taken against a pool that is not connected; `connect` fails after the acquire timeout. Recommendation: refuse it at wiring beside `LazyRefused`, naming both.

**P7. A store module cannot be found by its own type (F503).** `module_keyed::<SqlxModule<Postgres>, Primary>()` answers "no module `StoreModule<Postgres> @ Primary` is registered" because the identity is `of_value(&Identity<O>)` with a private `Identity`. The same private type appears as the hook key (`Identity<String>`), which a deployer reads. Recommendation: identify a keyed store by its module type and key, and fix the hook key in the docs-site pass as ruled.

## S-items that want a ruling

The catalogue's rule was that friction becomes an S-item and not a workaround. Over a hundred were filed; the checks classified each against the source as a real gap, an author error, or a table mismatch. The ones below need a decision. The rest are in the ledger and are mild.

**P8. `#[module]` has no slot for pre-dispatch metadata, qualified providers, readiness checks or closure hooks (B1, S5, S8).** Examples 03, 09, 17 and 34 drop to a hand-written `impl Module` or `DynamicModule`. B17 shows the global-enhancer path is designed (§7); the others are not. Options: (i) add the slots; (ii) keep `#[module]` small and say in the docs when to use `ModuleDef`. Recommendation: (ii) for readiness and hooks, which §9.1 designs out of the attribute on purpose; (i) for qualified providers and the pre-dispatch slot, which the examples needed.

**P9. The types a handler author must name are not re-exported where they are used (B2, B3, B7, D3).** `ulo-http` re-exports none of `Valid`, `Validate`, `Classify`, `CallError`, `ErrorKind` or `headers`; `ulo-rpc`'s `RpcError::kind()` returns `ulo_transport::ErrorKind` in a public signature with no re-export; `multer` types sit in public signatures. Every app adds `ulo-transport` and `headers` directly. Recommendation: re-export them from `ulo-http` and `ulo-rpc`. Cheap, and it removes a dependency no doc names.

**P10. No way to hand `Server` a ready rustls `ServerConfig` (B10), and no public scripted clock (S9, S18).** Client auth and custom certificate resolvers are impossible through `Tls::from_pem[_files]`. `Tls::from_config(Arc<ServerConfig>)` is the suggested shape. Separately, `ScriptedTimer` lives in a `publish = false` crate, so a user cannot copy examples 24, 28, 35 or 36, and a paused tokio clock jumps to a running server's own timers while a request is in flight (28 to 61 s measured). Recommendation: `Tls::from_config`; and decide whether a scripted clock is public API. If not, §3.1's promise of deterministic tests holds for the project and not for its users.

**P11. Small documentation gaps with a reproduced failure behind each.** B5 the `Sse` doc's `take_until(exec.draining())` is E0515 in a `'static` stream. G8 the `ulo_db_sqlx` doc shows `sqlx::migrate!` through the re-export and `macros` is not enabled. G14 `Otel::layer()` exports every span the subscriber passes, 170 for one request without a filter. D4 `Call::within` forwards the shorter of the caller's remaining time and the client's timeout; documented on `Call::timeout` and DESIGN:1116, not on `within`. E3 a gRPC guard answers UNAUTHENTICATED only through `Err(CallError::unauthorized(..).into())`; `Ok(false)` gives PERMISSION_DENIED. H5 `draining()` resolves before the servers stop accepting (the code's order; DESIGN §9.5 says "at the same moment"; `ExecutionRef::draining` and DESIGN.md:358 read looser). Recommendation: fix the wording in each place, and for H5 state the order the code has.

## What the inventory cross-check left open

Of `SWITCH_INVENTORY.md` §1's 26 gaps, 9 are fully covered by examples, 15 partly and 1 not at all (G23). Detail: scratchpad `crosscheck_inventory.md`, ledger entry of 2026-10-29.

**P12. G23, serve-loop death closing the app, has no cover and the inventory marks it unverified (Q6).** An example needs the behaviour pinned first. Is it intended, and is it tested somewhere the example can cite? Recommendation: pin it in a conformance test, then add an example row.

**P13. The partly-covered limbs: add them to existing examples, or leave them to the conformance suites?** Cheap: G7 guard-build ordering and several error handlers' order (ex-16), G13 a handler without `&self` and an aliased `Result` return (ex-15), G15 `#[meta]` on RPC (ex-25), G25 a connect guard's bag reaching `on_connect` (ex-23), G12 a `Form`, `Host` and request-side `BodyStream` (ex-15). Expensive: juniper, gRPC TLS, a same-schema-at-two-paths GraphQL case. Recommendation: add the cheap ones in one batch and list the expensive ones for after the merge.

**P14. Concepts from master with no home, and the per-host examples.** A custom extractor (`FromCall`), socket activation and pre-bound listener adoption (`LISTEN_FDS`), one guard type on several transports (`multi_protocol_context`), `error_telemetry`, and an HTTP handler pushing to WS clients (`gateway_http_bridge`) have no example and no §2 replacement. The four per-host files (`actix_poc`, `poem_poc`, `rocket_poc`, `salvo_poc`) name `crates/*/examples/` homes that do not exist (Q7). Example 28 serves six links and omits UDP; the `ulo new` template covers it. Recommendation: one example each for the first five, under the homes the cross-check suggests (ex-15, 16, 24 and a new socket-activation one); close Q7 by deleting the four per-host files from the inventory, since the adapters are tested by the HTTP conformance suites.

## CI

**P15. Two CI-only failures, each green on rerun and unexplained (F500).** `ulo-http-conformance` `nested::chunked_body_over_the_limit` ("Broken pipe" writing an over-limit body, `wire.rs:306`); `ulo-rpc-tcp` `conformance_tls` `a_call_held_past_a_spent_cap_is_answered_unavailable` (`abandoned` 2 where the test expects 1, `drain.rs:182`). Neither reproduced locally in the full gate or on earlier CI runs. The second is the one that matters: a count of 2 can mean a leaked execution, and P1 is one. Before the merge, probe both under load on Linux. Is that the right order, or should the merge wait on a diagnosis?

## For the record

- **Checks:** 39 examples, 2,493 tests, 0 failed, 174 ignored; every example's tests ran 30 times idle and 30 under twelve busy loops, 0 failures; builders and checkers together planted over 300 breaks in the examples, each failing by test name, with the misses closed by patches. Every example is public-API-only (a CI script fails on `__private` or a hidden item and its own self-test catches a planted violation), clippy `-D warnings`, and checked at its MSRV floor where the toolchain is installed (35, 36 and 37 declare 1.94, 1.94 and 1.89 and cannot be checked here; CI's MSRV legs cover them).
- **Not run:** the README commands that need `grpcurl` and `websocat`; examples 31 and 32 at 1.89.
- **Upstream:** async-nats and rumqttc are documented as measured properties of the pinned versions (transports §5.2); nothing is filed. `rumqttc-next` is filed as a post-merge candidate (F495).
- **What follows:** the docs-site pass, linking each example from its concept's page, and the merge over `master` as beta.
