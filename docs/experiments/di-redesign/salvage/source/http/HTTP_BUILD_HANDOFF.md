# Handoff — building the HTTP surface

For the session that implements what the HTTP audit decided. This file carries the process; the
plan is `HTTP_AUDIT.md`. Nothing here restates it.

## Read first, in this order

1. `HTTP_AUDIT.md` §7 — the eight decisions, all marked DECIDED, each with its rejected
   alternatives. **These are approved. They are not open.**
2. `HTTP_AUDIT.md` §10 — the build sequence, fourteen PRs, what each closes and the verification
   each needs.
3. ADRs **0052**, **0053**, **0054** on branch `docs/http-audit-adrs` (`9d3fb9c2`, off master
   `654c7183`).
4. `FRAMEWORK_GAPS.md` F105–F120 for the defects, and F4, F22, F27, F65, F69, F87, F103, F104 for
   the older ones the sequence closes. F243 and F244, filed by the render pass on 2026-09-23/24,
   are PRs 3–8's: an adapter never authors an error body. F117 is closed by the query-parser row
   `BUILD_SEQUENCE.md` adds after PR 3, not by any PR here.

## The gate

**Spent.** The pipeline audit ran 2026-09-20 and its outcome is at the top of `HTTP_AUDIT.md` §10:
`Dispatched` needs no third variant, one clause of ADR-0052's consequences is struck, and one
constraint is added to `dispatch`. Both land in PR 3. Read that outcome before starting it —
`PIPELINE_AUDIT.md` §9 carries the reasoning and F125 the probe behind the constraint.

The WebSocket, RPC and GraphQL audits touch nothing decided here and are not a gate.

## The branch

`docs/http-audit-adrs` holds the three ADRs and nothing else. **Rename it when work begins** — it
is named for what it holds today, not for what the work is:

```
git switch docs/http-audit-adrs
git branch -m feat/http-core-route-table    # or whatever the first PR does
```

Each ADR is `proposed`. The PR that implements one flips it to `accepted` and adds the supersession
pointers at the same moment — 0052 into ADR-0009 (the registration surface) and ADR-0007 (whose
rejected alternative it takes). An accepted record is not edited for a decision that has not been
taken, which is why those pointers are absent today.

## The divergence gate — stop and ask

Approval of this build is approval of **what was read and understood**. If the build has to depart
from that, it pauses and reports rather than proceeding on judgement.

What counts as divergence:

- A type or signature that differs from what the ADR writes.
- An alternative the ADR records as rejected turning out to be necessary.
- A variant, parameter or capability the ADR does not name.
- A finding that reorders the sequence, makes a PR unnecessary, or adds one.
- A verification that cannot be written the way §10 describes it.
- An adapter whose upgrade does not work at the dispatch point — the spike covered poem and salvo,
  and rocket already ships the shape; axum and actix are inferred, not probed.

What does not: naming details, file placement, test names, the order of two independent PRs.

On divergence: stop, state what the design said, what the tree requires, and what the options are.
Wait.

## The Law

Per `feedback-the-law-review-protocol` in memory: **before every commit message and every PR title
and body**, a dedicated agent reviews the text against `~/.claude/commits.md`,
`~/.claude/pull-requests.md`, `~/.claude/prose-discipline.md` and `~/.claude/posture.md`.

Run it **in the background** and surface only its findings — the full transcript is not wanted in
the session. Add per invocation: the branch and commit, an instruction to read the diff in full, a
numbered list of the technical claims to verify hardest, and how many prior passes found factual
errors. Ask for both facts and register; do not tell it the register scan is reliably clean.

A finding from the Law is a claim, not a verdict. Verify it before acting, and where it is wrong
about a mechanism while right about the substance, say so.

## Per-PR checklist

Walk it — the failure mode on record is a checklist written and not walked. It lives in
`feedback-the-law-review-protocol`. The two items most often missed here:

- **Production text this change falsifies** — doc comments, ADRs, and the workspace `CLAUDE.md`,
  which sits in the untracked parent where a repo-wide grep misses it.
- **One check in its plainest form, with no flags.** `--all-features` hides `cfg` bugs, and clippy
  cannot pass workspace-wide (F80).

Merge with `--merge`, never `--squash`.

## Bookkeeping

New findings go to `FRAMEWORK_GAPS.md` as `F<n>`, continuing from the highest number there. Do not
move `Last synced HEAD` or the `Docs status` section in `CLAUDE.md` — those are a documentation-site
audit's to move. Append one line per merged change to `DOCS_AUDIT.md` §18 naming the pages it
invalidates.

Findings that are about the pipeline rather than the wire belong to the pipeline audit's pass, not
this one, unless the build surfaces them — in which case they are this session's and file here.
