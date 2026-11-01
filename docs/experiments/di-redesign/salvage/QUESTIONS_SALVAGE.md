# Questions from the September Smile salvage (for the designer)

Written for: the designer. These are the questions that cross subjects or that a record raised as a decision, collected from the ten records in this folder. Each record's own "Worth discussing with the designer" section holds the rest, with the evidence. Cite these as QS-1 onwards. Ledger entries: `FRAMEWORK_GAPS.md` F524 to F561.

## Decisions the rebuild reversed or did not carry

**QS-1. The load-shed answer (gRPC F524, RPC F551, sequence).** The user-approved S3 ruling was `TooManyRequests` (gRPC RESOURCE_EXHAUSTED). HEAD answers `Unavailable`, with `RetryAfter` on RPC and nothing on gRPC. The only record of the change is a "for the record" line in `integrations/QUESTIONS_5.md:84`. A stream refused on a link without streams answers `internal` where S3 approved `Unimplemented`. Restore S3, or record why `Unavailable` is right.

**QS-2. WebSocket close codes (F532).** WS D4 settled on 3000, 3003 and 3008 from the IANA registry. HEAD maps Unauthorized and Forbidden to 1008 and Timeout to 1011. D5's rejected clamp also survives in `Connection::close` (F535), which turns an invalid code into 1011 with no reason and no log.

**QS-3. `%2F` in a path parameter (F552).** HTTP D2 kept it encoded so a decoded separator cannot become a traversal at a path join. HEAD decodes it, and `{*rest}` raises the same question for `..`. The recorded reason covers invalid UTF-8 only.

**QS-4. RPC delivery mode and frame limit (F551).** D2 gave a per-pattern delivery mode; HEAD has one per link, so an event cannot reach every instance on NATS, MQTT, AMQP or Kafka, while a Redis request runs the handler on every instance. D4 approved 1 MiB for the TCP frame; HEAD defaults to 4 MiB.

**QS-5. Middleware's role (sequence, pipeline).** Pipeline D2 settled one pre-dispatch role on all four transports; HEAD makes it HTTP-only.

**QS-6. gRPC status codes (performance record).** The old D4 let every transport name a code outside the eleven kinds through a `Custom` arm. HEAD keeps the source on gRPC (`with_grpc_code`) and fixes the kind table elsewhere. Confirm the loss on HTTP, RPC and WebSocket is intended.

## Gaps with a security or correctness shape

**QS-7. Route-scoped pre-dispatch leaks which routes exist (F537).** `apply_for::<ApiKeyAuth>(["/admin/*"])` answers an anonymous caller 404 for an unrouted `/admin/x` and 401 for a routed one. `ulo-http/tests/middleware.rs:225` pins it as intended. Recommended fix: a request-path positive scope on unscoped entries.

**QS-8. A body taken before the handler (F543).** After a guard or interceptor calls `HttpCx::take_body`, the handler answers 400 "`body` is missing" and an `Option<Json<T>>` becomes `None`. The application's bug is blamed on the client or hidden. `transports/DIVERGENCES.md:533` records it as intended.

**QS-9. A JSON body that parses but does not fit (F542).** It answers 400; RFC 9110 §15.5.21 says 422, and `serde_json` already tells the cases apart. GraphQL has the same shape (F528: every request error is 400 where the draft gives 422).

**QS-10. The Redis broadcast wire (F533).** The cross-process format is the derived serde shape of `Target` and `Audience`, unversioned and unpinned by any test. A new variant makes processes on either side of a rolling deploy drop each other's broadcasts. Version the wire, or pin its bytes in a test?

**QS-11. Unbounded RPC queues (F547, F548).** The client's reply queue and the server's request-stream feed are unbounded; `credit` is reserved and never sent. Per-call credit is deferred at HEAD. Build it, or state the unboundedness as a documented limit?

## Gaps the rebuild did not close

**QS-12. A handler is not told its caller left before the response head (F553).** gRPC fires `ClientCancelled` in the same situation. Probe whether hyper drops the service future on disconnect, then decide.

**QS-13. Module import cycles (F560).** HEAD refuses them. `MODULE_VISIBILITY_REVIEW.md` §2f rejected the refusal because an import edge is a visibility fact, and master pinned the tolerated shape. `SWITCH_INVENTORY.md` Q3 asks; the argument against refusing is not in the rebuild's record.

**QS-14. `StoreModule` identity (F558).** It covers the options only; `lazy`, health check, retries, backoff and the connect and migrate bounds sit outside it, and the first module reached under an identity wins silently.

**QS-15. Trailing metadata and the `grpc-` prefix (F525).** No API for trailing metadata; reply metadata screens eight names rather than the reserved prefix.

**QS-16. `ErrorKind::Timeout` renders as 504 (performance record, F69).** RFC 9110 defines 504 for a gateway or proxy whose upstream did not answer; a handler that overran its own deadline is not that. The question was never answered and the rebuild carried the mapping over.

## Decisions about method and records

**QS-17. Stale ADRs (F538, F546, F555).** ADR-0023 (`FromContext`) is `accepted` and describes a trait that no longer exists; ADR-0055 and 0056 are `proposed` and contradict the rebuild in eight places; ADRs 0052 to 0054 were never merged (restored here); 0057 to 0059 exist on `master` only. Whether `docs/adr/` survives the switch as a series, and how the rebuild's decisions are recorded, is open.

**QS-18. Hook order (sequence).** Master's #380 runs a module's own hooks before its providers'; HEAD's DESIGN §9.2 runs them after. Which governs?

**QS-19. Capability declarations (sequence).** HEAD has four declaration shapes where S10 wanted one.

**QS-20. Dropped markers and the missing `Content-Type` (extraction).** `#[body]`, `#[param]` and `#[query]` were dropped; the audit records the user calling them a deliberate alternative syntax. An absent `Content-Type` on a JSON body answers 415 where D4 chose "try to parse". These need the user, not the designer.

**QS-21. Reopening decisions the rebuild made moot (gRPC F527, performance).** The interop-suite "no" rested on "it would measure tonic"; ulo now owns the HTTP/2 server and the status and trailer writing. The benchmark surface, and which of four benchmark shapes to build, was never decided.

**QS-22. What to extract from the old ledger (F556).** About twenty entries the merged packets say to delete still stand, and `master` was merged without the ceremony. Reconcile them after the merge, or close them now with a note?
