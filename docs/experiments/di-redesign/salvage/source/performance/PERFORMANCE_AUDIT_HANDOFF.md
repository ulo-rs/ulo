# Handoff — the performance audit

Run this **last**, after the pipeline build and the DI audit have landed. Everything it would measure
today is about to change.

## Scope

Allocation and per-call cost across dispatch and the pipeline on all four transports, plus whatever
the measuring apparatus turns up on the way. Not correctness — every other pass owns that.

## Its first job is not measuring. It is building something to measure with.

**There is no benchmark surface in this repo.** No `benches/`, no criterion dependency, no CI job,
and no recorded number anywhere. So this pass cannot open by measuring: it opens by deciding what a
benchmark for this framework even is, and that decision is the user's.

The shape of the question: a framework benchmark can measure the framework or it can measure the
adapter under it, and separating the two is the hard part. Four candidates, in rough order of what
they cost to build:

1. **Dispatch without a socket.** ADR-0052 makes `ServeContext::dispatch` reachable with no adapter
   registered, so a benchmark can hand it an `HttpRequest` and read the `HttpResponse` — no kernel,
   no TCP, no adapter. This is the one that measures ulo rather than hyper, and 0052 is what makes
   it possible.
2. **Per-call allocation counts**, through a counting global allocator around one dispatch. Turns
   "this allocates per message" from a claim into a number, which is what F122 and F57 need.
3. **End-to-end through each adapter**, which measures five servers and ulo together. Useful for
   the adapter comparison the HTTP audit's table already started, and not useful for the pipeline.
4. **Compile-time and binary size**, which nobody has asked about and which macro-heavy frameworks
   pay for.

Answer that before writing a benchmark, and record the answer — a number with no stated apparatus is
not a measurement.

## Already filed — the claims waiting for a number

Every one of these is allocation-shaped, none is measured, and **no finding in this repo has been
filed on cost alone.** Keep it that way: a cost finding needs a number and the apparatus that
produced it.

- **F122** — every guard and every interceptor is constructed before the first guard is asked
  anything, so a refused request pays for the whole pipeline below it. Built in the pipeline build's
  Phase 1 PR 2, which makes this a *before and after* rather than an open question. Measure it if the
  apparatus exists by then.
- **F57** — three transports merge their enhancer tiers on every call: a `Vec` allocation plus one
  `Arc` increment per entry, then a hash lookup, then an extend, three times per message on RPC and
  WebSocket. Built in Phase 1 PR 4. Same before-and-after.
- **`project-async-trait-box-cost-note`** — a wait-and-watch note on `async_trait`'s per-call boxed
  futures, whose recorded trigger is *"a benchmark surface or stable `dyn AsyncTrait`."* **This pass
  is that trigger.** Read the note before proposing anything: the decision to accept the boxing was
  taken with its reasons, and this pass supplies the number that was missing, not a new argument.
- **AUDIT3_DISPATCH.md §9** records `async_trait`'s boxing of `T::Answer` as unmeasured, in the same
  breath as noting that the probe compiled it and nothing benchmarked it.

## What is different about this one

**It is the only pass whose findings can be wrong in a direction the others cannot.** A correctness
finding is falsified by a probe that contradicts it. A performance finding is falsified by a
measurement taken under different conditions, and the conditions are rarely written down. So:

- Every number carries its apparatus, its machine, its build profile and its commit.
- A comparison is run in one process against one build, never across two sessions.
- `--release` or it is not a measurement.
- A regression claim needs the same benchmark on both commits, not a remembered figure.

The confidence marking gains a category here: **Measured** means a number with its apparatus stated.
Anything else is Unverified, whatever it feels like.

## Deliverable

1. A decision from the user on which of the four benchmark shapes to build, taken before any code.
2. The apparatus itself, which is the durable part — it outlives every finding this pass produces.
3. A memo, `PERFORMANCE_AUDIT.md`. Form: `feedback-the-audit-memo-form`.
4. Findings to `FRAMEWORK_GAPS.md`, each with its number and apparatus.

Nothing is committed by an audit — with one exception to settle first: **whether the benchmark
apparatus is committed.** It is not a finding, it is infrastructure, and leaving it in a scratchpad
means the next pass rebuilds it. Ask.
