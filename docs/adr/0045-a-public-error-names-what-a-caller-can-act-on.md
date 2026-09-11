# 0045 — A public error names what a caller can act on

Status: proposed.

## Context

`anyhow::Error` appears in 95 places across `ulo`, and in the signature of every method that pulls
a provider or a module handle out of the container. A caller gets one opaque type back from
`get`, `get_by_token`, `resolve`, `get_module` and `ModuleRef`'s query, whatever went wrong.

The failure modes behind that type are closed and few: the provider is not registered, the module
is not imported, two modules share an identity base, the registered provider is not the requested
type, or the provider is request-scoped and there is no execution to build it in. Each has a
different fix, and two of them have a fix a program can apply rather than a person:

- An ambiguous module identity already prints the full keys that address the two modules, and
  `get_module_by_id` takes one of them. Inside a message string that recovery is available to a
  human reading a log and to nobody else.
- A request-scoped provider resolved without an execution is answered by opening one —
  `ProviderContext::standalone()` where the work arrived over no transport.

The cost is not only that a caller cannot branch. `instance_loader` retries deferred module
instantiation by testing `e.to_string().contains("DEFERRED:")`: a control-flow decision reading a
message body, which any edit to the wording breaks without failing a compile.

The framework has already applied this rule once, to one boundary. `InitResult` gave lifecycle hooks
`Box<dyn Error + Send + Sync>` in place of `anyhow::Result`, and the scanner decorates whatever a
hook returns with the module and hook names at the layer holding them. That removed `anyhow` from
one trait boundary users implement against. It was not carried to the others.

## Decision

**A public error names the cases a caller can act on, and where there are none it carries the
source and names nothing else.** The two halves apply to different surfaces, and which half a
surface gets is decided by one question: does any consumer branch on the value?

**Where a caller branches, the error is an enum.** Provider and module resolution is that surface.
`ResolutionError` replaces `anyhow::Error` on `UloApplication`, `UloApplicationContext` and
`ModuleRef`:

```rust
pub enum ResolutionError {
    ProviderNotFound { token: String, module: Option<String> },
    ModuleNotFound { id: String },
    AmbiguousModule { base: String, candidates: Vec<String> },
    TypeMismatch { token: String },
    RequestScopeOutsideExecution { token: String },
}
```

`module` is `Some` where one module was searched and `None` where every module was, which is the
distinction strict resolution draws. `candidates` holds the keys `get_module_by_id` accepts, so the
recovery the message used to describe is one the caller performs.

**Where nothing branches, the error is `Box<dyn Error + Send + Sync>` behind an alias, as
`InitResult` already is.** The adapter SPI is that surface: `bind` turns every adapter error into
`StartupError::Adapter { transport, source }` at nine call sites and inspects none of them. The
registration-before-acquisition ordering of ADR-0024 is carried by which method was called, not by
the value it returned.

`anyhow` stays in `ulo-cli`, which is a binary whose errors are printed.

## Consequences

- `get`, `get_by_token`, `get_from`, `get_from_by_token`, `resolve`, `resolve_by_token`,
  `get_module`, `get_module_by_id` and `ModuleRefQuery` return `ResolutionError`. A caller who
  wrote `anyhow::Result` around them still compiles: `anyhow::Error` converts from any
  `std::error::Error`.
- A test asserting a resolution failure matches a variant. Three in the suite matched substrings of
  a message and now do not.
- `StartupError` keeps `From<ResolutionError>`, so a startup path that resolves still uses `?`.
- An enum for the adapter SPI is refused, not deferred. It would ask eleven transport crates to
  classify roughly forty `?` sites into variants no consumer reads, and would let an adapter return
  a `Bind` failure from `register_route`, contradicting a fact the call site already holds.

## Roads not taken

**A newtype over `Box<dyn Error + Send + Sync>` for the adapter SPI.** It would exist to host
`msg()` and `context()`. `Err(format!("…").into())` already compiles through std's blanket
conversions, `.context()` appears in three crates, and a second carrier type beside `InitResult`
for the same kind of boundary is the inconsistency rather than the fix.

**One error type for the whole framework.** Resolution failures and adapter failures answer the
branching question differently, and a type covering both would carry variants that are unreachable
from half its call sites.

**Leaving resolution on `anyhow` and documenting the cases in prose.** The cases are already
documented in prose, in the message strings.
