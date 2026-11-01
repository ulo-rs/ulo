# Handoff — building what the pipeline audit decided

For the session that implements it. The plan is `PIPELINE_AUDIT.md`; nothing here restates it.

## Status

**Every decision is settled.** `PIPELINE_AUDIT.md` §10 records D1–D5, the API pass, the enhancer
grammar and the three "also settled" items, each with the alternatives rejected and why. They were
put to the user on 2026-09-21 and answered. **They are not open**, and a recommendation kept beside a
decision is there as its argument, not as a choice still to make.

The lifecycle the build works to is §5's diagram and §6's design. Read both before the first PR —
several rows below only make sense against them.

## Read first, in this order

1. `PIPELINE_AUDIT.md` §10 — the decisions.
2. `PIPELINE_AUDIT.md` §5 and §6 — the nesting, and the pre-dispatch role.
3. `PIPELINE_AUDIT.md` §1–§4, §7, §8 — the evidence, in the order a caller loses it.
4. `FRAMEWORK_GAPS.md` F121–F137 for what this closes, and F50, F51, F56, F57, F60, F73, F74, F81,
   F84, F118, F119 for the older entries it closes or spends. F242, filed by the render pass on
   2026-09-23, is built beside PR 8.
5. `AUDIT3_DISPATCH.md` §3, §4 and §8 before PR 4 — §4's measurement rule decides what stays per
   transport, and §8's open boxes are what that PR closes.

**Three older entries spell paths that #306 moved.** F56, F57 and F60 were verified before the
`dispatch` layer existed, and their line numbers predate it. Current locations:
`di/internal/instance_wrapper.rs` is now `crates/ulo/src/http/pipeline.rs`,
`di/internal/resolve/{rpc,gateway,grpc}.rs` is now `crates/ulo/src/dispatch/resolve/{rpc,ws,grpc}.rs`,
and `type_map.rs` is now `crates/ulo/src/context/type_map.rs`. The findings hold; only the paths
moved. F121–F137 were verified against `654c7183` and need no translation.

## The two ADRs

Written first, on their own branch, both `proposed`. Each is flipped to `accepted` by the PR that
implements it, which is also when its supersession pointers are added — an accepted record is not
edited for a decision that has not been taken.

- **The pipeline's order** — D1, D3, D4, the §8 vocabulary, the API pass, the enhancer grammar.
  Supersedes nothing. Carries the rule that decided two of its questions: *a trait carries what is
  true of everything that implements it; anything narrower goes on a narrower trait.*
- **The pre-dispatch role** — D2. Supersedes the middleware half of ADR-0007, and states the one
  behavioural change: module-middleware code moved to an interceptor moves *inside* the error chain.

## What is not this build's

- **F125** — a global middleware's failure reaching no error handler. It is the HTTP build's, inside
  ADR-0052's `dispatch`, and `HTTP_AUDIT.md` §10 already carries it as a constraint. Building it here
  means writing that function twice.
- **F84** — an extraction failure answering outside the chain. Goes to the extraction audit
  (`EXTRACTION_AUDIT_HANDOFF.md`); §11 records why extraction was not taken whole.
- **F22, F87, F65, F69** — the renderer, taken by ADR-0053 and by
  `RENDER_AUDIT_HANDOFF.md` for what 0053 leaves.

## The sequence

Three phases. Phase 1 is independent work that can start today and can run in any order. Phase 2
breaks the enhancer surface and its PRs land close together so an application migrates once. Phase 3
waits on the HTTP build.

### Phase 1 — independent

| # | PR | Closes |
| --- | --- | --- |
| 1 | One ordered enhancer vector, and the closure form as the third spelling | F121, F56 |
| 2 | Guards evaluated before any interceptor is built, and built one at a time | F122 |
| 3 | The stale texts, and the not-found diagnostic's article | F132 |
| 4 | `Resolved<T>`, the tiers pre-merged, `closure_leaf::<Grpc, _>`, one `safe_render` | F128, F129, F57, F74, F60 |
| 5 | One global registration surface per transport per role, taking a token or a value | F133, F134, F135 |
| 6 | A `Token<T>` const is nameable in `#[use_guards]`; the `APP_GUARD` example goes | F50 |

### Phase 2 — the enhancer surface breaks once

Order matters inside this phase. Run it as a block.

| # | PR | Closes | After |
| --- | --- | --- | --- |
| 7 | One error vocabulary: `Status { kind, message }` / `Custom { status, message }` / `AppError` on each transport | F136 | — |
| 8 | A guard's refusal is one event, on all four transports and both WebSocket phases | F130 | 7 |
| 9 | The API pass: `Transport` public and sealed, one type parameter on all three roles, `Next<T>`, context first, `catch`, one result alias, `HasHeaders` | F137, F127's doc half | 4 |
| 10 | A guard answers `Result<(), E>` | F131 | 8, 9 |
| 11 | Error handlers get the `Factory` arm; a non-singleton middleware is refused by a diagnostic naming the scope | F126, F51 | 9 |

### Phase 3 — after ADR-0052's route table exists

| # | PR | Closes | Gate |
| --- | --- | --- | --- |
| 12 | The pre-dispatch role: `Middleware` narrowed to one anchor, reaching all four transports, typed-error contract, path scoping over the framework table, tower mounted there, the module anchor retired | F73, F118, F119, F123, F124, F126's middleware half, F127 | HTTP build PR 2 (the core route table) |

PR 12 is the largest change an application sees. It is last because path scoping has nowhere to
resolve against until the table exists, and because PR 4 is what makes its wrapper edits one edit
rather than four.

## Verification each PR needs

Four probes were written and removed during the audit. Each is the test its PR owes, promoted from a
`println!` to an assertion.

- **PR 1** — a declaration mixing all three spellings runs in the written order; the same declaration
  with the spellings permuted runs in the permuted order. Cover the controller/method tier pair, and
  error handlers, where the probe found declaration order ignored outright. Add: a closure form
  builds per execution, and a value form does not.
- **PR 2** — an execution-scoped guard and interceptor below a rejecting guard record their
  constructors; the rejection means neither ran. Falsify with the same pipeline passing.
- **PR 5** — a global enhancer registered by token resolves with its dependencies on each of the four
  transports and for each of the four roles. Falsify by removing the provider.
- **PR 8** — a guard refusing a WebSocket *connect* and a guard refusing a *message* raise the same
  event and render the same kind.
- **PR 11** — `create` refuses an execution-scoped `Middleware` with a message naming the scope, and
  accepts an execution-scoped `ErrorHandler`, building one per execution. Falsify against singleton
  scope and against a guard.
- **PR 12** — a module middleware's domain error, moved to an interceptor, is claimable by
  `#[catch(MyError)]`; a pre-dispatch role's failure reaches the global handlers on all four
  transports; a pre-dispatch role sees a response a `#[catch]` handler produced, on all four. That
  last one is the probe the audit owed and did not run — read says yes on HTTP, and the other three
  have no anchor to read yet.

Every audit probe was disabled and re-run before being believed. A first clean run is not evidence;
`feedback-self-test-the-checker` is the rule and it has already caught one wrong probe in this
workspace, and one wrong citation during this audit's own verification pass.

## The branch

None exists. Nothing was committed by the audit and master is clean at `654c7183`.

## The divergence gate — stop and ask

Approval of this build is approval of what was read and understood. If the build has to depart from
that, it pauses and reports rather than proceeding on judgement.

Divergence: a signature that differs from what §10 describes; an alternative §10 records as rejected
turning out to be necessary; a finding that reorders the sequence, removes a PR or adds one; a
verification that cannot be written as above; a decision whose rejected alternatives turn out not to
have been the only ones.

Not divergence: naming, file placement, test names, the order of two PRs inside Phase 1.

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

Walk it. The recorded failure mode is a checklist written and not walked. It lives in
`feedback-the-law-review-protocol`. The two most often missed here:

- **Production text this change falsifies** — doc comments, the ADRs, and the workspace `CLAUDE.md`
  in the untracked parent, where a repo-wide grep misses it. Its *Execution order*, *Enhancers answer
  by returning*, *Errors* and *Globals* rows are all touched by this sequence, and two of them
  already carry a marker the audit added.
- **One check in its plainest form, with no flags.** `--all-features` hides `cfg` bugs and clippy
  cannot pass workspace-wide (F80).

Merge with `--merge`, never `--squash`.

## Bookkeeping

New findings go to `FRAMEWORK_GAPS.md` as `F<n>`, continuing from the highest there. Do not move
`Last synced HEAD` or the `Docs status` section in `CLAUDE.md` — a documentation-site audit moves
those. Append one line per merged change to `DOCS_AUDIT.md` §18 naming the pages it invalidates;
PRs 1, 8, 9, 10 and 12 each invalidate a docs page on enhancers, guards or middleware, and PR 12
invalidates every page that shows `configure_middleware`.

Findings about the wire rather than the pipeline belong to the transport's own audit unless this
build surfaces them, in which case they are this session's and file here.
