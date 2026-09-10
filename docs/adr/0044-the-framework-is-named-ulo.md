# 0044 — The framework is named ulo

Status: accepted.

## Context

This repository is a fork of `monterxto/toni-rs`. The `toni`, `toni-macros` and `toni-cli` names on
crates.io are owned by that account, whose last release was 2025-02 and whose maintainer has not
answered mail since March 2026. None of the other 26 crates in the workspace has ever been
published, so the framework had a name it could not publish under and 26 names nobody held.

crates.io is one flat namespace, first come first served, and it folds `-` and `_` so a name has one
spelling however it is written. [RFC 3243](https://github.com/rust-lang/rfcs/blob/master/text/3243-packages-as-optional-namespaces.md)
would add real namespacing — `ulo::axum`, with publish rights under `ulo` reserved to its owner —
but it has been accepted and unimplemented since 2022, and its
[tracking issue](https://github.com/rust-lang/rust/issues/122349) has been open since 2024-03. So a
prefix is convention with nothing behind it, and publishing is the only act that reserves anything.

That makes a rename cheap exactly once. A name published and then changed leaves the old one on
crates.io forever, pointing at a dead crate that nobody can reclaim or redirect.

The satellite names had a second problem the rename could settle at the same time. `toni-redis`,
`toni-redis-rpc` and `toni-redis-broadcast` were a database pool, an RPC transport and a WebSocket
backplane: three roles behind one middle word, with the role suffix present on two of them and
absent from the one that most needed it. Seven RPC transports were named after their brokers with
nothing saying they were transports at all.

## Decision

**The framework is `ulo`, and every integration crate names its role before its library.**

`ụlọ` is Igbo for *house*. NestJS, which this framework follows in shape, is a nest; this is a house,
and its modules are rooms.

Integration crates are `ulo-<role>-<library>`:

| role | crates |
| --- | --- |
| `http` | `ulo-http-axum`, `-actix`, `-salvo`, `-poem`, `-rocket` |
| `rpc` | `ulo-rpc-tcp`, `-udp`, `-nats`, `-redis`, `-rabbitmq`, `-mqtt`, `-kafka` |
| `db` | `ulo-db-seaorm`, `-sqlx`, `-diesel`, `-mongodb`, `-redis`, `-prisma` |
| `ws` | `ulo-ws-tungstenite`, `ulo-ws-redis` |
| `graphql` | `ulo-graphql-async-graphql`, `ulo-graphql-juniper` |

Crates that are not an integration keep a bare name: `ulo`, `ulo-macros`, `ulo-build`, `ulo-cli`,
`ulo-config`, `ulo-grpc`, `ulo-health`. The CLI binary is `ulo`, so the commands are `ulo new`,
`ulo generate resource` and `ulo dev`.

`toni-terminus` becomes `ulo-health`. Nest's `@nestjs/terminus` is a joke about a bus terminus that
carries no meaning on its own, and a role prefix scheme has no room for a name that does not say
what it is.

Crate directories move to a flat `crates/`. With the role in every name the directories sort into
their families on their own, so a nested `crates/rpc/ulo-rpc-nats` would state the role twice.

`LICENSE` keeps Antonio Carlos's copyright line, which MIT requires, and adds this fork's beneath it.

## Consequences

The 43 ADRs before this one were written while the framework was named `toni`. Their `toni::` paths
are rewritten to `ulo::` so they stay greppable against the code they explain; none of them is a
decision about the name, so nothing is lost by the rewrite, and this ADR is where the rename is
recorded.

`toni` 0.1.1, `toni-macros` 0.1.3 and `toni-cli` stay on crates.io under their existing owner. They
are not this framework and there is no migration path between them: the published versions predate
every architectural decision from ADR-0001 onward.

Nothing is reserved until it is published. The names are free as of 2026-09-11 and stay free to
anyone else until the first release, so the whole set is published together rather than one crate at
a time.
