# Handoff — the RPC wire audit

Run this as its own session. It gates nothing.

## Scope

The RPC wire across all seven transports — tcp, udp, nats, redis, rabbitmq, mqtt, kafka — against
the ADR-0032 stream grammar and against each broker's own published request-reply convention. The
standards audit named those four conventions as claimed and unchecked: AMQP `reply-to` /
`correlation-id`, MQTT v5 response-topic / correlation-data, NATS request-reply, and Kafka headers.
ulo is documented as following all four; none has been verified.

Out of scope: guards, interceptors and the error chain on RPC — the pipeline audit takes all four
transports at once.

## The thing to settle first

**`ulo-rpc-conformance` exists, and nothing runs it.** Each of the five broker transports invokes
`conformance_suite!` from a `tests/conformance.rs` carrying `#![cfg(feature = "integration")]`, so
`cargo test -p ulo-rpc-nats --test conformance` reports `ok. 0 passed` having compiled nothing, and
a plain `cargo check --workspace` does not compile them either. CI's `check (all features, all
targets)` job is the only thing that compiles the suite, and nothing anywhere runs it — deliberately,
since it needs five brokers under Docker.

So the conformance claims for five of the seven transports rest on a suite that has never executed.
Decide early whether this session stands up Docker and runs it, or whether it audits by reading and
says so in the confidence marking. **Either is legitimate; what is not is writing Probed for
something that was read.**

## Already filed — read before probing

- **F63** — the load shed answers `overloaded`, the one status that is not an `ErrorKind` name.
- **F64** — one RPC error object spells its two fields in two cases.
- **F1** — an extraction failure renders `Internal` on RPC and WebSocket, with a diagnosis and a
  build plan already written under the entry.
- **F86** — an RPC handler must return a `Result` where HTTP and WebSocket may return a bare value.
- **F46** — RPC dispatches by string match inside one controller object where HTTP dispatches per
  handler.
- `STANDARDS_AUDIT.md` §5 on JSON-RPC 2.0, whose finding decides the shape of any adoption: **the
  spec is silent on the two things ADR-0032 built.** Streaming replies and cancellation have no
  counterpart in JSON-RPC, so adopting it means extending it, and an extension carrying a standard's
  name has borrowed credibility. What maps cleanly: the correlation `id`, notification-as-absent-`id`,
  and transport-agnosticism.

## What the HTTP pass learned that transfers

The HTTP audit's finding was that **divergence tracks who decides**. Where the answer came from the
underlying server, five adapters agreed; where it came from the adapter crate, they did not. Seven
RPC transports each write their own framing in a `wire.rs`, and `AUDIT3_DISPATCH.md` §4 already
recorded that the four broker `wire.rs` files differ by 148 of 156 lines — which it called a genuine
seam, correctly drawn. Test that: the question is whether the *shared* grammar is shared in fact.

Under `project-the-spec-conformance-seam`, most of the RPC wire is category (3) — ulo's own
invention, since no standard owns a generic message broker RPC. That makes coherence the test, not
conformance, **except** where a broker's own convention is category (1) and ulo claims to follow it.

## Method

- Probe, then falsify. Fetch specs with `curl`, not from memory. Confidence marking per claim.
- Brokers under Docker are the cost of a Probed tag here. Budget for it or mark Read.

## Deliverable

1. `RPC_AUDIT.md` at this level. Form: `feedback-the-audit-memo-form` in memory.
2. Findings to `FRAMEWORK_GAPS.md`.
3. Its own build handoff.
