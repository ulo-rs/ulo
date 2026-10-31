# DI redesign (experiment)

An end-to-end redesign of ulo's dependency injection, written from a list of required capabilities
without reference to the previous implementation, and the rebuild of the framework to it.

On this branch the whole workspace is built to `DESIGN.md`: the core (`crates/ulo`, `crates/ulo-macros`),
the transports (`transports/DESIGN.md`) and the integrations (`integrations/DESIGN.md`). The workspace
tests pass, and `examples/` holds 41 runnable packages, one concept each, listed in the root `README.md`.

- `CAPABILITIES.md` — the 48 capabilities and the constraints the design answers. The bracketed
  numbers in the design, such as [12], refer to this list.
- `DESIGN.md` — the design. `fw` stands for the framework's crate prefix, `ulo`.
- `REFINEMENTS.md` — the first review of the design, each claim marked by how it was established.
- `SHUTDOWN_REVIEW.md` — the review of the shutdown phases, per-hook timeouts and terminal
  executions.
- `RESPONSE.md` — the design author's responses to the reviews and to the rebuild's divergences, in
  order, each with the user's sign-off.
- `BUILD_PLAN.md` — the rebuild's areas, the files each owns, and the contracts between them.
- `DIVERGENCES.md` — every place the rebuild departs from the design or fills a gap it leaves,
  grouped by wave, with the user's decisions on each.
- `divergences/` — the per-area logs `DIVERGENCES.md` is assembled from.
- `transports/CAPABILITIES.md` — the brief for the transport layer, the second race: what it must do on top of the DI core, with no reference to the previous implementation.
- `probes/` — the scratch crate behind the reviews, outside the ulo workspace; each binary shows
  one claim, and a `*_fails.rs` binary's compile error is its result.
- `integrations/DESIGN.md` — the integration layer: configuration, health, stores, telemetry and the CLI.
- `SWITCH_INVENTORY.md` — what master did that the rebuild has to cover or drop, row by row, with the
  example or test that covers each.
- `DESIGNER_HANDOFF.md` and `CORE_TEST_PLAN.md` — the hand-off from the design to the build, and the
  core's test plan.
- `QUESTIONS_*.md` (under `transports/` and `integrations/`) — the open questions after each build
  round, each with the recommendation put to the design author.
