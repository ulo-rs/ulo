# Questions after the Q21 rulings and the merge batch are built and checked (for the designer)

Written for: the designer. The Q21 rulings and the merge batch from the salvage rulings (QS-3, QS-4's capability, QS-7 on HTTP and gRPC) are built, checked by a second model, gated (2,862 tests, no failure) and committed (`4f5738aa`, `eb4584db`, `a5283747`); CI on the branch is green. Cite these as Q22-P1 onwards. None of them gates the merge to `master`; each is to be settled before `0.1.0-alpha.2`. Detail: `FRAMEWORK_GAPS.md` F522, F562 to F566.

Paste alongside: transports `DESIGN.md` §2.7, §3.2, §3.3, §3.4, §3.8, §5.3, §6.2.

## Rulings needed

**Q22-P1. Should a scoped pre-dispatch entry run before the built-in gRPC health and reflection services?** QS-7 makes a scope match the request path. An entry scoped `/*` therefore covers an unrouted method, but not the health and reflection services, which run no scoped entry (DESIGN §6.2 states this on purpose). With reflection on, an anonymous caller still lists every service and method; reflection is on by default in debug builds. Running scoped entries before the built-ins puts health probes behind auth, and an app can protect reflection today with an unscoped entry. Options: scoped entries cover both; cover reflection only; keep as is and say so in the reflection docs. Recommendation: cover reflection, exempt health.

**Q22-P2. A valid configuration is refused at `listen()` on a host without `host_extensions` (F565).** Routes `/admin/health` and `/admin/{page}` (reading `Host<Tenant>`), with `apply_for::<S>(["/admin/*"]).exclude(["/admin/health"]).supplies::<Tenant>()`: under rocket every request that reaches `/admin/{page}` passes the entry, because the static route claims `/admin/health` for every method, yet the check counts only entries that run on every request of a route and refuses, saying the scope "does not cover `/admin/{page}`". HEAD before QS-7 accepted it. Options: make the check ignore exclusion overlap at paths a higher-precedence route claims; or keep the refusal as the cost of per-request scopes. Recommendation: fix the check.

**Q22-P3. `\` in a single-segment parameter (QS-3).** The claim that `root.join(value)` cannot climb out of `root` holds on Unix. `%5C` decodes to `\`, which a Windows `Path::join` treats as a separator. Options: refuse `\` and `%5C` in a single-segment value with 400; or document the guarantee as Unix-only. Recommendation: refuse, since the same sentence promises the rule to every join.

**Q22-P4. The ALPN rule for a list naming no transport protocol (Q21-P3).** The first build supplied the transport's protocols for any list naming none of them, so `http/1.1` alone on gRPC, `h2` alone on WebSocket and `h3` alone on HTTP were supplied. The checker read the ruling's parenthetical, "only `acme-tls/1`, or nothing", as limiting supply to an empty or acme-only list, and refuses the rest as missing a protocol. The second reading is applied. Confirm.

## Confirmations

**Q22-P5. What QS-7 changed beyond the ruling.** Each follows from "a scope matches the request path":
- A scoped entry also runs before an upgrade handler.
- The prepare-time refusal's criterion is widened: a scope is refused when it shares no path with any route, or when its own exclusion contains it. It was: when it covers no route whole. A scope narrower than a parameter route (`/users/me` under `/users/{id}`), once refused, now starts and is checked per request.
- An embedding with `Miss::Forward` gets a scoped entry's refusal before the forward decision, so it pre-empts the host's own route under that path.
- The dot-segment 400 carries no CORS headers, because it precedes every entry.
- On gRPC, `Stage::unrouted` is public in `ulo-http` and used by `ulo-grpc`, so a `/*` scope no longer matches the path `/` there.

**Q22-P6. What QS-3 reads.** `%252F` decodes once, so it reaches the handler as `%2F`, the same text as a client-sent `%2F`; the handler cannot tell them apart. `{*rest}` keeps `%2F` encoded too, since every `/` in a rest value is one the client sent raw. A static route segment is stored decoded, so a route and a scope written in different percent-encodings agree (the checker found a routed request skipping its scope when they differed). Confirm.

**Q22-P7. The shape of the request-delivery capability (QS-4).** `Capabilities` carries `delivers_requests_once: bool` (default true) and `fan_out_reason: Option<&'static str>`, set through `fans_out_requests(reason)`. The server warns once from `prepare`, refusing nothing. A link can set the bool false without a reason; the warning then reads "the link declares it". This is a fifth capability shape for QS-19's comparison. `DeliveryMode`'s documentation ("how a request reaches two server instances") overlaps and wants revisiting when per-pattern event delivery returns. On tcp and udp the new conformance scenario runs one instance, so it shows only that a request runs once on one server there. Accept the shape?

## For the record

**Q22-P8. F522.** Measured as pre-existing: `a_call_held_past_a_spent_cap_is_answered_unavailable` on RabbitMQ failed 1 of 90 runs on the current tree and 2 of 90 on the tree before Q20, always as the reported `Timeout`. Likely mechanism, from reading: `RabbitMq::close` does not wait for the server writer to publish the queued "shutting down" answer before closing the connection. Candidate fix: drain the writer and close the connection in one detached task. Not built; not a merge gate.

**Q22-P9. QS-20 decisions.** The `#[body]`, `#[param]` and `#[query]` markers stay dropped, the user's reason being that a macro attribute on a function parameter is an unnatural, unrusty spot, the type already says what the parameter is, and the old markers had their own bugs. A missing `Content-Type` on a JSON body keeps answering 415; a per-route relaxation is added only if users hit it.

**Q22-P10. Panics still reported as ordinary errors (F562).** The NATS and MQTT roots loads become `Misconfigured` with an unredacted panic text (decisions 139 and 147 record it as deliberate); the long-lived lane, writer and reply-router tasks on every broker link end silently. Q21-P6 ruled a panic is never an ordinary error. Sweep all links after the merge, or leave the deliberate ones?
