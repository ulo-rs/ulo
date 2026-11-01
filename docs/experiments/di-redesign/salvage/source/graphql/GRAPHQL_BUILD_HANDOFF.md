# Handoff — building what the GraphQL audit decided

For the session that implements it. The plan is `GRAPHQL_AUDIT.md`; nothing here restates it.

## Status

**Every decision is settled.** `GRAPHQL_AUDIT.md` §10 records D1–D8, each with the alternatives
rejected and why. They were put to the user on 2026-09-22 and answered across that session. **They
are not open**, and a recommendation kept beside a decision is there as its argument, not as a
choice still to make. Three of them record a position the audit itself argued for and withdrew —
D1's `294` collapse, D3's engine-asymmetry framing, D8's consistency-with-`HttpContext` argument.
Those are rejected alternatives, not open questions.

The shape the build works to is §7 and §8: **conformance is a property of each binding, ulo owns
every binding and the resolver seam, and no engine is ever written.** Read both before the first PR.
Several rows below only make sense against them.

## Read first, in this order

1. `GRAPHQL_AUDIT.md` §10 — the decisions.
2. `GRAPHQL_AUDIT.md` §7 and §8 — the per-binding rule, and what ulo owns.
3. `GRAPHQL_AUDIT.md` §9 — what the resolver seam was tested against, and the two corrections.
4. `GRAPHQL_AUDIT.md` §1–§6 — the evidence, in the order a caller loses it. §1's and §3's tables are
   the conformance suites PRs 5 and 6 owe, case for case.
5. `FRAMEWORK_GAPS.md` F182–F195 for what this closes.
6. `HTTP_AUDIT.md` §1 before PR 3 — its `AdapterCapabilities` is the shape D7 takes.
7. `project-the-spec-conformance-seam` in memory, the 2026-09-22 append — the rule §7 generalises.

**Fetch the three protocol documents rather than recalling them.** GraphQL-over-HTTP from the
`graphql-over-http` repository's `spec/GraphQLOverHTTP.md`, graphql-transport-ws and graphql-sse from
their `PROTOCOL.md` files. The audit did, and one fetched summary carried a detail that read as a
fabrication and was not.

## The two ADRs

Written first, on their own branch, both `proposed`. Each is flipped to `accepted` by the PR that
implements it. Numbered when written — 0052 and 0053 are reserved by the HTTP and render builds, so
these take the next free pair.

- **A GraphQL engine is an SPI; its bindings are ulo's.** D1, D5, D6, D7, D8. Carries §7's
  constraint as its motivation: two published specifications decide the same question and disagree,
  so a status rule in a shared layer makes some binding non-conformant by construction. States that
  an engine answers `Items<Response, E>` and never a status, a media type or a frame.
- **A resolver is a handler.** D3's Path B, D4. Supersedes nothing. States the one thing an
  application sees: a resolver gains DI, guards, interceptors, metadata and `#[catch]`, and
  `ContextBuilder` is deleted rather than deprecated (D13's pre-release rule).

## What is not this build's

- **F190's adapter half** — three HTTP adapters read a list-valued `Sec-WebSocket-Protocol` as one
  token, which reaches every gateway. It belongs to the **`ws/`** pass, which has not run. PR 6
  closes the gateway half only, and a client offering two subprotocols still fails at the handshake
  until the adapter half is built.
- **The DI ambiguity under F191** — first import wins over an `FxHashSet`, reporting nothing, is the
  **DI build's**. PR 4 removes this subject's instance of it by deleting the shared token; it does
  not fix the container.
- **Incremental delivery** — `@defer`/`@stream` is Stage 2, absent from the September 2025 spec
  edition, and its HTTP binding is still an RFC. Not built. §7 says why the SPI costs nothing to be
  ready for it: a subscription already forces `Items::Many`.
- **F4 and ADR-0052** — the duplicate-route panic. Not in the tree at `654c7183`; F4 stands.
- **Owning an execution engine.** §8 settles it with a number: 35,125 lines in async-graphql, 42,572
  in juniper, 23 validation rules in async-graphql's `rules/` alone. No PR here writes a parser, a
  validator or an executor.

## Where the code goes

A new **`ulo-graphql`** crate, beside `ulo-grpc` and named the same way: it holds the SPI, the
neutral request and response types, the request-error taxonomy, and every binding. The two existing
crates become engine implementations and stop holding controllers, gateways and context builders.
Core gains nothing — GraphQL is not a transport and `ulo` must not learn about it.

## The sequence

Three phases. Phase 1 is free-standing and can start today. Phase 2 is the engine seam and the
bindings, and Path A works throughout it. Phase 3 is the resolver seam and depends on Phase 2.

### Phase 1 — free-standing

| # | PR | Closes |
| --- | --- | --- |
| 1 | The playground is served as `text/html`, both crates | F185 |
| 2 | Delete the juniper crate's header comment, which contradicts the ledger it cites | F193 |

**Two, not six.** An earlier reading counted F184, F186, F194 and F190's gateway half here on the
grounds that they need none of the design. They do not need it and are still not free-standing:
each is a defect in request parsing or in the ws gateway, and both are replaced in Phase 2. Fixing
them first is work the rewrite discards, and the crate is pre-release with nobody to buy it for.
They are owed by the PRs that replace the code, and are listed there.

### Phase 2 — the engine seam and the bindings

Order matters. PR 3 is the seam everything else mounts on.

| # | PR | Closes | After |
| --- | --- | --- | --- |
| 3 | ADR A, and `ulo-graphql`: `GraphqlEngine`, `EngineCapabilities`, the neutral request/response types, `RequestErrorKind`, `ModuleRef` as the resolution handle | — | — |
| 4 | The two engine implementations; `GraphQLModule<E>` with `with_schema` (Path A) and the engine on the module | F191, F192 | 3 |
| 5 | The HTTP binding: `Accept` negotiation, `application/graphql-response+json`, the §6.4 status table, `Content-Type`, `406`, `415`, `294` | F182, F183, F184, F186, F194 | 4 |
| 6 | The graphql-transport-ws binding: connection state, the five close codes, `error` frames, routing by operation type | F187, F188, F189, F190's gateway half | 4 |
| 7 | The graphql-sse binding, both modes | — | 5 |

**PR 3 carries D5's obligation and it is the one row that cannot be deferred.** §1 shows the
request-error/field-error distinction cannot be read off a serialised response: `Response` declares
`pub data: Value` with no `skip_serializing_if`, and `is_ok()` is false for field errors too. The
SPI carries the taxonomy or PR 5 cannot write a status. juniper's `GraphQLError` answers it exactly;
the async-graphql implementation reconstructs it — `parser::parse_query` is public, validation is
not, so parse failures are exact and validation failures rest on `errors[].path` being empty. That
the two answer with different precision is what the SPI is for, and PR 4 is where the difference
gets written down.

**PR 7 is where the per-binding rule pays or the design was wrong.** graphql-sse MUST answer a
validation error `200` plus a `next` event where PR 5 answers `422`. If the same taxonomy cannot
produce both from one engine result, §7 is falsified and the build pauses under the divergence gate.

### Phase 3 — the resolver seam

| # | PR | Closes | After |
| --- | --- | --- | --- |
| 8 | ADR B, the `builds_schema` capability, and the dynamic-schema builder registering one closure per field into ulo's pipeline | F195 | 3, 4 |
| 9 | The declaration macros — `#[graphql_object]`, `#[query]`, `#[mutation]`, `#[subscription]`, `#[arg]` — and the Rust-type-to-GraphQL-type mapping | — | 8 |
| 10 | `ContextBuilder` deleted; both `with_auth` examples rewritten onto guards and DI | — | 9 |

PR 8 is the one the probes already cover. PR 9 is the only unproven work in the build:
§9 records that the macro and the type mapping were **not** tested, by design, on the grounds that
they are engineering if the closure works. That is a reason to sequence them last, not a claim that
they are easy.

juniper declares `builds_schema: false` and backs Path A only. That is D3 working as intended and
needs no PR.

## Verification each PR needs

Six probe files are in `artifacts/resolver-seam-probes/` beside this handoff, copied out of the
session scratchpad on 2026-09-23 because that directory is session-scoped and under `/private/tmp`. Each is the test its PR
owes, promoted from a `println!` to an assertion. The four probes from the audit's first half —
HTTP conformance, transport-ws, the token collision, two schemas at two paths — were **not** kept,
and are rewritten from §1's and §3's tables, which carry every case.

- **PR 4** — a consumer importing two GraphQL modules resolves the engine its own module holds.
  Falsify with the pre-fix shape, where `imports: [alpha(), beta()]` panics and `[beta(), alpha()]`
  answers the other module's service with no diagnostic.
- **PR 5** — §1's table as a conformance suite, one assertion per row, run against **both** engines.
  The `405`/`Allow` row already passes and stays in as a regression guard. Add the legacy-client
  path: `Accept: application/json` gets §6.1's treatment, status codes intact and `Content-Type`
  replaced on `2xx`.
- **PR 6** — §3's table as a conformance suite, one assertion per rule, plus: a query sent over the
  socket executes rather than answering `"Subscription root not found"`. That last is §9's second
  correction and is the case a binding meets first, because graphql-transport-ws calls it a *single
  result operation*.
- **PR 7** — a validation error answers `200` with a `next` event carrying the errors, against PR 5
  answering `422` for the same document. Both from one engine result. This is the falsifier for §7.
- **PR 8** — `probe_resolver_seam.rs`, `probe_resolver_seam2.rs` and `probe_resolver_race.rs`
  promoted. The race probe **must keep its control**: the same test with `scope = "transient"`
  produces 24 builds and fires the assertion, which is what makes the clean run evidence.
  `probe_error_path.rs` and `probe_sub_error.rs` pin the typed error reaching `#[catch]`, with their
  no-source control.
- **PR 10** — the two `with_auth` examples answer the same thing before and after, with the
  authentication moved from the context builder to a guard.

Every probe was disabled and re-run before being believed. A first clean run is not evidence;
`feedback-self-test-the-checker` is the rule, and in this audit alone it caught one wrong probe
(§9's `execute_stream` correction) and one wrong ledger claim (F184's block on F136).

## The branch

None exists. Nothing was committed by the audit and master is clean at `654c7183`.

## The divergence gate — stop and ask

Approval of this build is approval of what was read and understood. If the build has to depart from
that, it pauses and reports rather than proceeding on judgement.

Divergence: an SPI signature that differs from what §10 describes; an alternative §10 records as
rejected turning out to be necessary; PR 7 unable to answer both bindings from one taxonomy;
the closure seam failing where §9 says it holds; a verification that cannot be written as above.

Not divergence: naming, file placement, test names, the order of PRs 1 and 2, whether PR 9 is one
macro PR or three.

On divergence: state what the memo said, what the tree requires, and what the options are. Wait.

## The Law

Per `feedback-the-law-review-protocol`: before every commit message and every PR title and body, a
dedicated agent reviews the text against `~/.claude/commits.md`, `~/.claude/pull-requests.md`,
`~/.claude/prose-discipline.md` and `~/.claude/posture.md`. Run it in the background and surface only
its findings. Give it the branch and commit, an instruction to read the diff in full, a numbered list
of the technical claims to verify hardest, and how many prior passes found factual errors. Ask for
both facts and register.

A finding from the Law is a claim, not a verdict.

## Per-PR checklist

Walk it. The recorded failure mode is a checklist written and not walked, in
`feedback-the-law-review-protocol`. The two most often missed here:

- **Production text this change falsifies** — doc comments, the ADRs, and the workspace `CLAUDE.md`
  in the untracked parent, where a repo-wide grep misses it. Its **GraphQL** row is falsified by
  PRs 4 through 10 and its **Naming** row gains a crate; PR 10 falsifies every mention of
  `ContextBuilder` and `with_subscription_context`.
- **One check in its plainest form, with no flags.** `--all-features` hides `cfg` bugs and clippy
  cannot pass workspace-wide (F80).

Merge with `--merge`, never `--squash`.

## Bookkeeping

New findings go to `FRAMEWORK_GAPS.md` as `F<n>`, continuing from the highest there. Do not move
`Last synced HEAD` or the `Docs status` section in `CLAUDE.md` — a documentation-site audit moves
those.

Append one line per merged change to `DOCS_AUDIT.md` §18 naming the pages it invalidates. The docs
site carries three GraphQL pages — `content/docs/graphql/index.mdx`, `async-graphql.mdx` and
`juniper.mdx` — and every PR from 4 onward invalidates at least one of them. PR 9 invalidates all
three outright, because the declaration syntax every example on them shows is no longer the
recommended one.

A new crate needs its row in `integration-tests/tests/integration/coverage_ledger.rs` before
`the_ledger_is_complete` passes; that is PR 3's, not a follow-up.
