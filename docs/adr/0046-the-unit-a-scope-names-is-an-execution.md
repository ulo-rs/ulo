# 0046 — The unit a scope names is an execution

Status: accepted.

## Context

`#[injectable(scope = "request")]` builds one instance per execution and shares it within that
execution. ADR-0016 settled what an execution is: one HTTP request, one WebSocket message, one RPC
or gRPC call, or one standalone execution opened by hand.

The name says request, and the places that document it exist to say it does not mean one:

- `ProviderScope::Request`'s own doc opens *"Created once per execution and shared within it — one
  HTTP request, one WebSocket message, one RPC or gRPC call."*
- ADR-0016 is titled *context spans one execution*, not one request, and settles the unit in those
  words.

The rest of the framework already reads the other way. `ExecutionCache` is the per-execution
instance store, `ExecutionResult` is what a dispatch returns, `ProviderContext`'s doc opens *"The
execution a provider is being built for"*, and `injector/module_ref.rs` binds the value as
`execution: ProviderContext::None`. The word "execution" appears 153 times in `ulo`'s prose.

The divergence is not cosmetic. ADR-0022 added `ProviderContext::standalone()` — an execution with
no transport behind it, for a CLI command, a job or a test. A provider built for one is
request-scoped with no request anywhere in the program.

## Decision

`ProviderScope::Request` is `ProviderScope::Execution`, and `scope = "request"` is
`scope = "execution"`. The macro crate's `ControllerScope::Request` follows.

`scope = "request"` is refused at compile time with an error naming the new spelling and the unit it
means. It is not accepted as an alias.

The test this applies, which decides the cases either way:

> A conventional name survives when the convention's meaning is what this framework does.

Spring, NestJS and .NET all mean *the app as a DI root* by `ApplicationContext`, and ulo's
`UloApplicationContext` is the app as a DI root, so that name stays. Spring, NestJS and Java CDI all
mean *one incoming request* by request scope, and ulo's unit is larger than that, so this one goes.

The tell in both directions is the corrective line. `ApplicationContext` needs none anywhere.
`scope = "request"` needs one wherever it is documented.

## Consequences

Breaking for every `#[injectable(scope = "request")]`, `#[controller(scope = "request")]` and
`provider_factory!(scope = "request")`, and for anything matching on `ProviderScope::Request`. The
refusal names the replacement, so the fix is mechanical and the compiler finds every site.

The three scopes read `singleton`, `execution`, `transient`. Two are adjectives and one is the unit
it scopes to, which is a grammatical mismatch the old trio did not have — `request` was a noun too,
so the mismatch predates this and is not introduced by it.

## Roads not taken

**Keep `"request"` as a deprecated alias in `FromStr`.** Costs nothing to implement and leaves two
spellings for one thing, which is what PR #291 spent 289 files removing from the crate's paths. The
framework is pre-1.0 and this series is already breaking.

**`per_execution`.** Keeps strict parallel with the two adjectives and reads worse at the call site.

**`scoped`, after .NET.** Names that there is a scope without naming what it is, which is the
question the old name got wrong.

**Rename `ProviderScope` itself.** It is the scope of a provider and says so. Only the variant was
wrong.
