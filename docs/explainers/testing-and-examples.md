# Where a test goes, and what earns one

The workspace holds twenty-nine crates. Five implement `HttpAdapter`, seven speak the RPC wire
grammar, six wrap a database. That shape decides how testing works here: the question is rarely
whether a behaviour is worth pinning, it is which of twenty-nine places the pin belongs in, and
whether pinning it once can cover five crates instead of one.

This page answers both. [ADR 0007](../adr/0007-pre-routing-global-chain.md) records the conformance
suite as a decision about the global middleware chain; the pattern generalises beyond it.

## The rule

**A test belongs where its owner lives.** The question that locates it: *whose change should break
this test?*

One crate's change → that crate. Any of five crates' changes → where all five are visible. Every case
below is that question applied.

## Three homes

| Home | Holds | Breaks when |
| --- | --- | --- |
| `crates/X/src/**` under `#[cfg(test)]` | Pure logic whose edge cases are awkward to reach from outside — parsers, graphs, mappings, state machines | That algorithm changes |
| `crates/X/tests/` | What X does that no other crate does: its deviations, its quirks, and the service-backed suites behind its `integration` feature | X changes |
| `integration-tests/` | Behaviour that does not exist until crates are wired together, and conformance suites over a trait with more than one implementor | Any participant changes |

The third is the one that scales, and it is the one a reader coming from a single-crate project does
not expect.

## Unit tests, and the advice that gets garbled

"Don't unit test a library" is a mangled version of something true. The true form is **don't let a
test reach into private items**, because a refactor that preserves behaviour then breaks it, and the
test has pinned the implementation rather than the contract.

Rust gives that rule structural support. A file in `tests/` links the crate as an external consumer,
so it can see `pub` items and nothing else — the same view a user has. A `#[cfg(test)] mod tests`
inside `src/` sees everything, which is the temptation.

So the rule is not about where the test lives, it is about what it can see:

- Reach for `#[cfg(test)]` when the subject is pure logic whose interesting inputs are painful to
  drive from outside. A malformed wire frame, a route pattern with a mid-segment colon, a dependency
  graph with a three-node cycle.
- Reach for `tests/` whenever the public API gets you there.

`ulo`'s unit tests sit on the first side of that line: `rpc/wire.rs` covers the frame grammar and its
malformed cases, `injector/dependency_graph.rs` covers cycle detection, `middleware/route_pattern.rs`
the matcher, `grpc_status.rs` the code mapping, `extractors/path.rs` the typed parse. Driving a
truncated frame through a real socket to test the parser would cost a server and prove less.

The inverse also holds: nothing unit-tests the DI container's storage, and nothing should. What a
consumer promises against is that a singleton is constructed once and a transient once per injection,
which is asserted over a running application in `di_core.rs`.

## What earns a test

Two filters, applied to a test one at a time:

1. **Would a refactor that preserves behaviour break this?** Then it tests implementation. Rewrite or
   delete it.
2. **Would a real bug go undetected without it?** Then it earns its place. Otherwise it is redundant.

Applied to this framework, the recurring shapes:

| Don't pin | Pin instead |
| --- | --- |
| That a macro generates a particular item name | That the generated code routes a request |
| That an enhancer's trait impl was detected | That the guard answers 403 |
| That the container stores providers in a map | That scope decides how many instances exist |
| That axum routes, NATS delivers, Postgres stores | That *this adapter's* translation into them is right |

The last row is the one a framework gets wrong most easily. An integration crate's whole job is
translation — subject naming, header mapping, body adaptation, the cancel carrier — and a test that
proves the upstream library works has proved nothing about the translation.

The first two rows have a worked example in the history: two files once probed the autoref detection
mechanism directly while `#[use_guards]` was still being threaded through the controller macro. Once
that shipped, a behaviour-preserving change to detection would have broken them while breaking
nothing a user could observe. They were deleted; `marker_free_enhancers.rs` pins the contract.

## Conformance suites

**A trait with more than one implementor gets one suite instantiated per implementor, not one suite
per implementor.**

The HTTP side is built this way. Four files in `integration-tests/` hold the contract once and a
`macro_rules!` stamps it out per adapter:

```rust
conformance_suite!(axum, ulo_http_axum::AxumAdapter::new());
conformance_suite!(poem, ulo_http_poem::PoemAdapter::new());
conformance_suite!(salvo, ulo_http_salvo::SalvoAdapter::new());
conformance_suite!(actix, ulo_http_actix::ActixAdapter::new());
conformance_suite!(rocket, ulo_http_rocket::RocketAdapter::new());
```

Seventy tests come out of those four files: thirty from the global chain, twenty-five from trailing
slashes, ten from `{param}` syntax, five from listener adoption. A sixth adapter is one line each,
and it either passes or it is not an adapter. Where an implementor cannot satisfy the
contract, the exception is written into the suite rather than omitted from it — rocket cannot adopt a
pre-bound listener, so `bind_target_conformance.rs` requires it to refuse at `bind()` rather than
binding somewhere else.

### What the alternative costs

The RPC side is not built this way, and the cost is measurable in the tree today. Redis, MQTT,
RabbitMQ and Kafka each carry a hand-maintained `round_trip.rs` with the same seven functions.
Normalise the broker's name and only 110 of roughly 255 lines differ between two of them, nearly all
of it container setup.

Seven copies of a contract diverge, and nothing notices, because divergence is only visible against a
single source:

| | send / emit / metadata | streams and cancels | reconnect |
| --- | --- | --- | --- |
| redis, mqtt, rabbitmq | yes | yes | yes |
| kafka | yes | yes | **no** |
| nats | **no** | yes | **no** |

NATS is missing the send/emit/metadata round trip entirely, not merely a reconnect suite. No test
failed when it went missing, because there was no suite for it to be missing *from*.

The service-backed half of those tests still needs Docker and stays in each crate behind its
`integration` feature — the shared suite is hermetic and must stay that way. What moves is the
*cases*: one macro the broker crates each instantiate against their own container, so a transport
that skips a case fails to compile.

## Examples

An example answers **one question a user would ask**. That is the whole test; the rest follows
from it.

- The root `examples/` crate holds questions about the framework: how to write a guard, how to
  validate input, how to shut down gracefully.
- `crates/X/examples/` holds questions about X: how to connect to this database, how to mount this
  schema. It is also mandatory where X's derive macros emit crate-anchored paths, which is why the two
  GraphQL crates keep theirs — `::async_graphql` and `::juniper` resolve only against a direct
  dependency.
- One concept per file. A file that needs section dividers to stay navigable is two files.
- A crate a user installs deliberately needs at least one example; the entry point is the question
  they have. A crate that only exists inside the framework — `ulo-macros`, `ulo-build` — needs none.

Assertions belong in an example wherever the outcome is checkable. An example that prints the wrong
answer confidently is worse than no example, and an assertion turns it into a test that also
documents.

## Measuring coverage without fooling yourself

The two filters judge tests one at a time, which finds redundancy and never finds a gap. Gaps are
found in the other direction: **enumerate what a surface promises — every usage mode, every ordering
guarantee, every failure and interruption path — then map tests onto the promises.** A promise with no
test is the missing test; a test pinning no promise is the redundant one.

`integration-tests/tests/integration/coverage_ledger.rs` answers a narrower question. It proves
every crate is tested *somewhere* and that the bookkeeping about where is true. It cannot tell
whether those tests pin anything worth pinning. That judgment is the per-surface pass above, and one
surface at a time is enough.

## Adding a crate

Four steps, in order. The first three decide where its tests go; the fourth records the decision.

1. **Does it implement a framework trait that other crates also implement?** Add a line to the
   relevant conformance suite. Its own `tests/` then covers only what it does differently, and that
   file is short by construction.
2. **Does it wrap an external service?** Its suite goes in `crates/X/tests/` behind an `integration`
   feature, started by testcontainers. It does not join the shared suite, which stays hermetic and
   runs on every pull request.
3. **Is it pure logic?** `#[cfg(test)]` under `src/`, plus the shared suite if its output only means
   anything inside a running application — that is why `ulo-macros` is registered there despite
   having no `tests/` of its own.
4. **Record it** in `coverage_ledger.rs`. The test fails until the line is there and matches the
   tree.

A crate that reaches step 4 with nothing to declare is a crate with no tests, which the ledger accepts
only against a written reason.
