# Handoff — the gRPC audit

Run this as its own session. It gates nothing, and it runs **after the RPC wire audit** by the
user's decision on 2026-09-23 — the aim was to settle RPC first so this pass has a settled
neighbour to compare against rather than two moving surfaces.

## Scope

gRPC end to end: `ulo-grpc` (the adapter, ~1 000 lines), `ulo::grpc` (context, status, extractors,
the macro ABI), `ulo-build` (proto compilation and the `{service}_ulo` shape markers, 489 lines),
and `#[grpc_methods]` in `ulo-macros`. Against **the gRPC specification**, against tonic's own
contract, and against what the other three transports do.

Out of scope, each already taken:

- **Guards, interceptors and the error chain.** The pipeline audit took all four transports at once
  and settled them. Its F129 and F130 both name gRPC and belong to that build.
- **Extraction.** ADR-0043's params are `FromContext<GrpcContext>`, and the extraction pass owns
  that trait. **Check what it claimed on gRPC before probing** — if it covered the gRPC context,
  this pass takes only what is left.
- **Rendering.** `RENDER_AUDIT_HANDOFF.md` owns the renderer seam, and it records that gRPC is the
  one transport with no fallback literal.

## The thing to settle first — how much of this is tonic's

**gRPC differs from every other transport in this campaign in one way that changes the whole
method: it has a specification, and a dependency that implements most of it.**

The HTTP/2 framing, the `application/grpc` content type, length-prefixed messages, trailers,
`grpc-status` and `grpc-message`, flow control and keepalive are all tonic's, and auditing them
audits tonic. **The pass is worth running only for the seam above that**, and deciding where the
seam is, is its first job.

A workable cut, to be confirmed rather than assumed:

- **tonic's**: wire framing, HTTP/2, trailer mechanics, the `Status` type's encoding, TLS.
- **ulo's**: which status code a failure becomes, what a handler may take and return, whether an
  execution is cancelled and when, what metadata a handler can read and write, what `add_service`
  bypasses, what `ulo-build` generates, and the shape of `#[grpc_methods]`.

If the honest answer after a day is "tonic does it and ulo adds four things, three of which are
fine", say that and write a short memo. An audit that pads itself to justify its existence is worse
than none — the render brief says the same thing for its own reason.

## Already filed — read before probing

- **F75** — a wrong `Payload<T>` on a gRPC handler fails on the first call rather than at compile
  time. **Parked by the user on 2026-09-16** with the words *"let us have a look at this when it
  allows"*; `project-grpc-parked-followups` holds it. Do not build it unprompted; do say whether the
  audit changes the cost.
- **F129** — gRPC keeps a second interceptor chain, and the renderer seam is three copies. The
  pipeline build owns it.
- **F130** — the same guard returning `false` is `Forbidden` on a message and `Unauthorized` on a
  WebSocket connect, and gRPC writes its own third answer. **This is a specification question on
  gRPC and an invention on the other two**, which is exactly the kind of split this pass exists to
  find.
- **ADRs 0033, 0037, 0038, 0039, 0042, 0043, 0048** are gRPC's own record, and 0049 and 0050 touch
  it. 0042 is superseded by 0043 — read the pointer convention in
  `project-adr-refinement-convention` before assuming either is current.
- **Four parked memory entries**: `project-grpc-parked-followups`,
  `project-grpc-handler-dx-parity` (approved in principle 2026-09-02, unbuilt — the user asked for
  "comparative DX"), `project-grpc-method-marker-followup`, `project-grpc-out-of-scope-capabilities`.
  Each names a trigger. A pass that re-proposes one of them without reading why it was parked is
  spending the user's time twice.

## What the RPC pass learned, and how little of it transfers

`RPC_AUDIT.md`'s finding was that **the framing was written down once and everything it did not name
was decided seven times and disagreed.** That shape needs seven adapters. gRPC has one, and a
specification above it, so the finding does not transfer and looking for it will waste the pass.

What does transfer is the *method*, and three specific comparisons where gRPC is the control:

- **gRPC already does two things RPC does not**, and the audit should confirm they work rather than
  assume it: `with_drain_timeout`, `with_max_inflight` and `with_max_per_connection` all exist on
  `GrpcAdapter`, where the five RPC brokers have none of the three.
- **gRPC reads a deadline off the wire and RPC cannot.** `CLAUDE.md` says *"it reports, tonic
  enforces"*. **Unverified, and worth being the first probe**: does a handler's execution token fire
  when `grpc-timeout` expires, or does the caller get a `DEADLINE_EXCEEDED` while the handler runs
  on? The RPC audit found the same class of leak on all seven transports (F222) and gRPC is where
  the framework claims to have solved it.
- **gRPC answers `ResourceExhausted` where RPC answers `overloaded`** (F63, F223). One of the two is
  the specification's word and the other is not, which decides F63.

## What is likely to be there — candidates, not findings

Marked **Unverified**. Each is a question the brief noticed and did not establish; a pass that
reports them without probing has not run.

- **Metadata is a flat `HashMap<String, String>`** (`crates/ulo/src/grpc/context.rs:144`). gRPC
  metadata permits repeated keys and **`-bin`-suffixed keys carrying arbitrary bytes**. What happens
  to a binary metadata value, and to a repeated key, is unknown here. The RPC pass found the flat-map
  question on four brokers and each resolved it differently; gRPC is where a specification decides
  it.
- **Reserved keys.** `grpc-` is a reserved metadata prefix. Whether ulo prevents an application from
  setting one, and what happens if it does, is unchecked.
- **`add_service` bypasses the pipeline entirely** — documented, and the only seam in the framework
  where a registered handler runs with no guards, interceptors or error chain. Whether that is a
  position or an accident is a question for the memo.
- **`ulo-build`** is a build-time surface with its own failure modes: a vendored `protoc` that
  yields to `PROTOC`, a `{service}_ulo` module appended beside `{service}_server`, and three ways to
  point at a proto compiled elsewhere. What a wrong one reports is unchecked.
- **Cancellation on a unary call.** ADR-0033 makes a streaming reply carry the execution and fire
  its token when abandoned. A unary call abandoned by the caller is the same question the RPC pass
  answered "nothing happens" for (F222).

## The one thing gRPC has that nothing else in this campaign does

**An official interoperability suite.** gRPC publishes `grpc-testing` protos and a set of interop
cases (`empty_unary`, `large_unary`, `client_streaming`, `server_streaming`, `ping_pong`,
`cancel_after_begin`, `cancel_after_first_response`, `timeout_on_sleeping_server`, status and
metadata cases) that every implementation is expected to pass.

The RPC pass's sharpest lesson was that `ulo-rpc-conformance` existed, had never run, and passed the
day it was run — so the suite was right and nearly empty. gRPC's equivalent is **written by someone
else and cannot be nearly empty**, which makes it the strongest single instrument available to this
pass. Fourteen `grpc_*.rs` files exist in `integration-tests` and none of them is that suite.

Deciding whether to stand it up is a first-day call, not an end-of-pass wish.

## Method

- **Probe, then falsify.** gRPC needs no Docker: a tonic client against a ulo server in-process is
  the whole harness, and `integration-tests` already has fourteen files doing it. **There is no
  excuse for a Read tag on anything a probe could answer here**, which is not true of the broker
  transports.
- **Fetch the specification with `curl`, not from memory.** The wire protocol and the status-code
  guidance are separate documents and the second is the one that decides F130 and F63.

```
curl -sS https://raw.githubusercontent.com/grpc/grpc/master/doc/PROTOCOL-HTTP2.md
curl -sS https://raw.githubusercontent.com/grpc/grpc/master/doc/statuscodes.md
curl -sS https://raw.githubusercontent.com/grpc/grpc/master/doc/keepalive.md
```

- **Confidence marking per claim** — Probed, Read, Spec, Unverified. The form is
  `feedback-the-audit-memo-form` in memory, and `RPC_AUDIT.md` is the most recent worked example.
- **Separate what tonic decides from what ulo decides, per finding.** A defect in tonic is a
  dependency note; a defect in ulo is a finding. Filing the first as the second is how this pass
  goes wrong.
- **Read the library before writing a mechanism.** `feedback-a-probe-shows-what-not-why` — the
  WebSocket pass filed two findings with a plausible wrong mechanism and corrected them the same
  session. tonic is a large dependency and the temptation is greater here.

## Deliverable

1. `GRPC_AUDIT.md` at this level. Form: `feedback-the-audit-memo-form` in memory.
2. Findings to `FRAMEWORK_GAPS.md`, continuing from the highest `F<n>` there (F234 as of
   2026-09-23).
3. Its own build handoff, **only if there is a build** — see the first section. A short memo saying
   "tonic does it and the seam is sound" is a legitimate outcome and a better one than a padded
   sequence.
4. Any probe or spike copied into `artifacts/` beside the memo. The session scratchpad is
   session-scoped and under `/private/tmp`; six handoffs in this campaign pointed at files there
   until 2026-09-23.
