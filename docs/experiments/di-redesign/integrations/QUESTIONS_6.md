# Questions after the tenth response is built and checked (for the designer)

Written for: the designer. Every ruling in the tenth response is built, checked by a second model and on `experiment/di-redesign` at `45bc3016`. CI is green across all 38 jobs. The workspace runs 2055 tests with no failure. Each build was followed by an independent check that planted semantic breaks; together they found and closed more than 100 weak tests and 12 behaviour defects (listed at the end). The items below need a ruling or a confirmation. Each carries a recommendation. Detail: `CORE_TEST_PLAN.md` (status paragraphs R1, R2, R3a, R3b), `FRAMEWORK_GAPS.md` F458 to F474.

Paste alongside: core `DESIGN.md` §3.4, §9.5, §10.1; transports `DESIGN.md` §2.6, decision 56.

## Drain behaviour

**E1. A drain landing just before the response head is flushed closes the connection with nothing written.** An endless stream answered within about 8 ms of the drain window's end, 200 runs: 101 clients got the 503, 74 a cut-off 200, **25 read zero bytes then EOF**. No panics, and never a 503 appended to a 200. The service had produced the response, hyper had not flushed the head, `ExecBody`'s first poll saw the drain and emitted the error frame, and hyper closed with neither the head nor the 503. A known-length body showed only 200 or 503.
- Options: (a) accept: an EOF is a cut like any other; (b) the service checks the drain once more as it returns a streaming response and answers 503 instead, which narrows the window to the hand-off to hyper; (c) a backend hook so a body can ask whether its head has gone out.
- Recommendation: (b), and (a) for the residual window; state it in §2.6.

**E2. Three transports poll a drain differently.** HTTP checks the drain before the pipeline (§2.6), so a handler that answers in reaction to its cancellation loses; gRPC and WebSocket poll the pipeline first and deliver that answer; RPC polls cancellation first, so an answer after the handler's own explicit cancel is lost, where gRPC and WebSocket deliver it. All three are documented and tested as they stand. Which is the rule? Recommendation: the pipeline first everywhere, so a handler that has chosen its answer is honoured; amend §2.6 and RPC.

**E3. A spent shutdown cap leaves every held call a reset (F466).** With `shutdown_timeout(ZERO)`, `close` returns in about 0.1 ms and reports the calls abandoned, but no client sees the B3 rendering: WebSocket "connection reset without closing handshake", gRPC `Unknown "transport error"`, RPC `Unavailable "the link closed before the reply arrived"` (the client's own phrase). `close` is polled once under a spent cap and the renderings are written after that poll.
- Recommendation: write the abandon rendering in the first poll of `close` where it needs no wait (send the gRPC status, the 1001 frame, the RPC answer, the HTTP 503), and let the 500 ms flush wait stay cut by the cap.

**E4. The graces add up across servers.** `close_servers` closes servers one after another, so a frozen WebSocket client and a frozen gRPC client each holding a call made `close` take about 1.3 s: 300 ms drain, then 500 ms plus 500 ms of grace. Each server is bounded, the app is not bounded by one grace. Recommendation: close servers concurrently, so the grace is paid once.

**E5. `HyperOn::close` has no abort guard like gRPC's `Closing`.** Under a cap remainder shorter than the 500 ms flush wait, `serve.close()` never completes and the failure is reported as an HTTP `Close` failure `TimedOut(ShutdownCap)` every time. Nothing leaks. Recommendation: give it the guard, and report a close cut by the cap as skipped, not failed.

## HTTP

**E6. A malformed multipart body propagated with `?` answers 500 "internal error".** C11 mapped the size case to 413; any other multer error is a client error reported as the server's fault. Recommendation: map malformed-body errors to a `Malformed` extract error and 400.

**E7. F458: `PrepareError` carries stage failures as text only.** The C12 test can only match the `file:line:` and the backticked scopes in the report text. Recommendation: structured entries. Give `Failure` (in `ulo-transport/src/prepare.rs`) an optional `location` and a `subject`; add `PrepareError::entries()` whose entries offer `names()`, `location()` and `scopes()`; `Stage::unreached` returns entries instead of strings.

**E8. C12 reaches less than the ruling says.** It refuses per entry, so an entry `["/admin/*", "/typo"]` is accepted; the reach flag is set before exclusions apply, so an entry whose covered routes are all excluded is accepted; and gRPC's scoped `PreDispatch` is never checked, because `unreached` is `pub(crate)` in `ulo-http` and `ulo-grpc/src/server.rs` builds its own stage. Recommendation: per scope, exclusions applied before the flag, and the same check for gRPC.

## Core

**E9. The controller-read refusal covers bindings only.** It fires for a binding, a global enhancer binding and a contribution reading `Dep<Ctl>`. It does not fire for a module hook reading `Dep<Ctl>`, a handler's enhancer closure (`guard_with(|c: Dep<Ctl>| ..)`), a handler parameter reading another controller, metadata values, or a runtime lookup (`app.get::<Ctl>()` returns the controller). Capability 29 and DESIGN now say "bindings only". Confirm, or extend to module hooks and enhancer closures. Recommendation: extend to module hooks and enhancer closures; leave a controller reading a controller, and runtime lookups, allowed.

**E10. The timeout knobs accept a zero (F470).** `.construct_timeout(ZERO)` and `.hook_timeout(ZERO)` pass `wire()`; an item that awaits anything then fails at connect with "timed out after 0ns (the app default)", while an item that finishes on its first poll connects. A3 refuses a zero on items. Recommendation: refuse a zero knob in `wire()` with a `ZeroBound` item kind of its own.

**E11. `Option<Dep<dyn Trait>>` as an `#[injectable]` field or constructor parameter fails E0477 (F469).** "Must satisfy the static lifetime" from `construct`'s `+ Send`, even with `+ 'static` written. `Dep<dyn Trait>` and `Many<dyn Trait>` compile. The checker found it identical under the parent's macro; the builder did not rebuild the parent to confirm. Fix, or document it as a limit?

**E12. Dead code in `ulo` (F445, 17 warnings).** `timer.rs::timeout`, `register_if_secret` and a handful of items in `binding/handle.rs`, `module/def.rs` and `redact.rs` are unused in the library; the pure tests exercise `timeout`. Delete, or wire?

## Stores and OpenTelemetry

**E13. The MySQL client certificate (`ssl-cert`, `ssl-key`) is read only by SQLx, in every TLS mode.** `create` checks only the root certificate, and only in verifying modes. A missing client certificate is now classified `Permanent` over TCP at the first ping, which matches C4, but is not refused in `create`. Recommendation: leave it; the classification is enough.

**E14. Under `reqwest-blocking`, `ureq` is still compiled** because it is not an optional dependency, and the two clients differ on the platform proxy: reqwest's `system-proxy` feature adds the macOS SCDynamicStore and Windows registry proxy settings when `HTTP_PROXY`/`HTTPS_PROXY` are unset, which `ureq` never reads. Recommendation: make the two clients mutually exclusive features so one HTTP stack compiles.

## Test-process confirmations

**E15. Four confirmations carried from earlier batches** (none was ruled):
1. Five trybuild cases have only a case-side break (`constructor_not_send`, `dep_of_a_non_send_type`, `trait_binding_non_implementor`, `enhancer_without_its_role_trait`, `controller_enhancer_for_another_transport`); no one-line change to the product removes their refusal. Accept?
2. Tests may check `Display` and `Debug` for a secret's absence, an exception to "no message text".
3. `LoadRefusal::Contribution` fires when a lazy module contributes to a collection the graph already has or reads.
4. Variant-only assertions remain in `grpc_tls.rs`: `TlsError::Rustls(_)` does not assert which rustls error, and `Read { path, .. }` does not assert the I/O kind; `Certificate(_)` and `PrivateKey(_)` carry free text only. Give `TlsError` structured members?

## For the record: defects found by the checks and closed

- **The macro probes were plain-named trait methods** called with method syntax in the user's module. A user's blanket trait with a same-named method made every `#[injectable]` ambiguous (E0034) or, for X24's metadata check (`check`) and gRPC's request-message check (`matches`), silently turned the refusal off, so a handler the check exists to refuse compiled. Every probe is now `__ulo_*` and inherent calls go by path (F471).
- **`reqwest-blocking` was built with no TLS backend,** so an `https` endpoint went out in plaintext, through a proxy or not. It now carries a ring and platform-verifier config like the `ureq` client (F474).
- **SQLx's Postgres reach check read only `get_socket()`** and missed a `PGHOST` directory, so a socket not yet created gave a permanent `NotFound`; SeaORM treated every connection as a Unix socket. Both read how the pool connects (F472, F473).
- **A gateway reading an `Ext` that only a message guard writes failed every connection with close 1011** because `dispatch` always built the handler's controller (F468).
- **`is_panic` did not recognise a panicking `prepare`.**
- **A zero drain window never entered a server's drain** (F455); **async-graphql answered a missing required variable as an executed result** (F456); **`Valid<Payload<T>>` did not compile on an RPC handler** (F457); **`AfterInit` ran twice with two HTTP servers** (F450); **a WebSocket message pushed between the loop's last `take` and the close decision was discarded** (F463); **hyper's `close` aborted connections before the client read the 503** (F459).
- **Two premises did not hold when built.** The chunked-body "defect" was the `/stream` test route catching its own `TooLarge`; every host answers 413. The Redis retry count of 3 was no better than 0.
- **The lesson that repeated across 100+ tests:** a test that asserts the variant but not which module, key, route, header, line, reason or transport it names stays green under a break that names the wrong one. The rule you set is in force in every brief.
