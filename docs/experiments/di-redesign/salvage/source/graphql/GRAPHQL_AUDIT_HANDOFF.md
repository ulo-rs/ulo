# Handoff — the GraphQL audit

Run this as its own session. It is the smallest of the four and gates nothing.

## Scope

`ulo-graphql-async-graphql` and `ulo-graphql-juniper` against the specifications that own the
surface: the **GraphQL over HTTP** specification for the request and response shape, and
**graphql-transport-ws** for subscriptions. Both integrations mount through ulo's HTTP surface, so
the question is what ulo adds, what it passes through, and where the two diverge from each other
and from the spec.

Out of scope: the GraphQL language itself, and the enhancer pipeline.

## What is already known

- Module identity is `of_type::<Self>().fingerprinted(config)` (ADR-0029): identical imports dedup,
  the same schema at two paths mounts both, and colliding on one path is refused by a native
  duplicate-route unwind — which is **F4**, a panic out of `bind()` rather than a `StartupError`.
  ADR-0052 closes F4 by moving duplicate detection onto a framework-owned table, so check whether
  this is still reproducible before filing anything about it.
- The two integrations are not at parity: async-graphql has WS subscriptions with the
  `graphql-transport-ws` subprotocol enforced; juniper has none and responds with
  `serde_json::Value`.
- `event_field()` — the override that makes a gateway read `"type"` rather than ulo's event field,
  which is what graphql-ws needs — requires a manual `Gateway` impl, with no macro knob.
- `WsError::Refused { code, reason }` exists for a close-only protocol refusal, and ADR-0036 names
  graphql-ws's 4406 as the case it was built for.

## Questions worth opening with

- Does the HTTP transport match the GraphQL-over-HTTP spec on the things it specifies: the
  `application/graphql-response+json` media type, status codes for a request that fails to parse
  versus one that fails to execute, and `GET` versus `POST` handling?
- Does the subscription transport implement `graphql-transport-ws` completely — `connection_init`
  timeouts, `ping`/`pong`, `subscribe`/`next`/`complete`, and the close codes the protocol defines?
- Where the two integrations answer differently for the same schema, is the difference the
  library's or ulo's?

Under `project-the-spec-conformance-seam`, the GraphQL-over-HTTP and graphql-transport-ws documents
are category (1): they decide, and ulo conforms. That makes this pass closer in shape to the HTTP
audit than to the pipeline one.

## Method

Probe, then falsify. Fetch the specifications rather than recalling them — both are published on
`graphql.github.io` and the protocol document lives in the `graphql-ws` repository. Confidence
marking per claim.

## Deliverable

1. `GRAPHQL_AUDIT.md` at this level. Form: `feedback-the-audit-memo-form` in memory.
2. Findings to `FRAMEWORK_GAPS.md`.
3. Its own build handoff, if it finds enough to warrant one.
