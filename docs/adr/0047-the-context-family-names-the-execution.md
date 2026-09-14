# 0047 — The context family names the execution

Status: accepted.

## Context

`Context` is the suffix on three unrelated things.

The first is one execution: `HandlerContext`, the interface; `HttpContext`, `RpcContext`,
`WsContext`, `GrpcContext` and `StandaloneContext`, which implement it; and `ProviderContext`, an
enum over those five plus a `None`. The second is `ServeContext`, handed to an HTTP adapter once per
serving session. The third is `UloApplicationContext`, the application as a DI root. Only the first
group is one concept, and two names inside it are wrong.

**`HandlerContext` names one of five users.** Guards, interceptors, error handlers, providers and
extractors all hold one; the handler is not privileged among them. The trait's own doc calls it
*"the universal interface every per-request context implements"*, which is what it is, and the name
does not say so.

**`ProviderContext` names its parameter position.** It is what `Provider::resolve` takes, and its
doc opens *"The execution a provider is being built for."* Everything else about it already reads
that way: `ExecutionCache` is the store it hands out, `injector/module_ref.rs` binds the value as
`execution:`, and ADR-0046 made the scope that uses it `ProviderScope::Execution`.

## Decision

`HandlerContext` is `ExecutionContext`. `ProviderContext` is `Execution`.

The trait takes the `Context` suffix because that is what its implementors are — a per-transport
view of one execution. The enum takes the bare noun because it is the execution, erased: which one
is running, or none.

The per-transport contexts keep their prefixes. `HttpContext` is written beside `RpcContext` in
`impl Guard<HttpContext>`, where the prefix disambiguates at the use site — the test ADR-0046's
series applied to the rest of the crate.

`di::Execution` and `di::ProviderScope::Execution` are the same word in one module on purpose: the
variant names the unit a scope is bound to, and the type is that unit.

## Consequences

Breaking for every `HandlerContext` bound a caller writes and every `ProviderContext` a caller
names, which includes the nine integration crates that build one to resolve against: `ulo-config`,
both GraphQL crates, and all six database crates.

`ServeContext` and `UloApplicationContext` keep their names. The first is a context in the ambient
sense and now sits in `ulo::http` beside the `HttpContext` it is not (#297). The second is
load-bearing convention for the app as a DI root — Spring, NestJS and .NET all mean that by
`ApplicationContext`, which is what ulo's is, and ADR-0046's test keeps a conventional name whose
meaning matches.

## Roads not taken

**`Context` for the erased enum.** The crate has no unified context type and says so; naming the
enum `Context` would announce the opposite.

**`ExecutionContext` for the enum, leaving the trait alone.** Puts the better name on the narrower
thing and leaves `HandlerContext` naming one of its five users.

**`Execution::Outside` in place of `Execution::None`.** More literally correct, since an execution
that is none is a contradiction. `Option::None` has trained every Rust reader to parse `None` as
absence, and 35 call sites already read that way.
