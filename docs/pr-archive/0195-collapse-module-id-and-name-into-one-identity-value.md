# #195 — Collapse module id and name into one identity value

Merged 2026-08-27 into `master` from `refactor/one-module-identity`, commit [`b97479b`](https://github.com/ulo-rs/ulo/commit/b97479b6924d7fbbbbf5533f91020e9b5b5cef09).

A module has one identity: `ModuleIdentity { base, fingerprint }`, returned by the single trait method `identity()` that replaces `get_id` and `get_name`. The rendered key — `base` or `base#<16 hex digits>` — is the registry token, the display string, and an address. Implements ADR-0029.

The name channel carried no information the id lacked, and where it was supposed to help it failed. It was hand-written per module in formats that never agreed (bare ident, `ConfigModule<full::path>`, the constant `"GraphQLModule"`, the builder-given base). The export-clash refusal printed the two colliding modules' display names — identical for two configs of one maker — while the fingerprinted ids that distinguished them went unshown. By-name lookup could not separate two same-type GraphQL modules, which share the name.

- **Core** — `ModuleIdentity` with `of_type::<M>()` / `named(base)` / `fingerprinted(config)`; the hasher lives there once, replacing the copies in `DynamicModule` and both GraphQL crates. `Display` prints the key; the scan log and the clash refusal show it. `Module` drops its stored name and the dead token/name accessors.
- **Lookup** — `get_module_by_id` replaces `get_module_by_name`: a full key matches exactly, a bare base (a `DynamicModule`'s builder-given name, a type path) matches with ambiguity detection, and the keys an ambiguity error lists are themselves valid input. `get_module::<M>()` matches on the parsed base instead of string prefixes.
- **Macros / crates** — the `#[module]` emission, `DynamicModule`, `CheckedModule`, the builtins, toni-config, and both GraphQL modules implement `identity()`.
- **Tests** — key/parse round-trips; the module-identity suite asserts through `identity()`; `module_lookup` gains the address test: a module value built with the same config renders the key that reaches the imported one of two same-type GraphQL modules — the case neither type nor name could previously address.

Display is the key, deliberately: verbose-and-unambiguous over short-and-collapsing. A derived `short()` for logs is deferred until log noise is real.

Breaking for hand-written `ModuleMetadata` impls: implement `identity()` instead of two methods. The fingerprint hash is deterministic across runs of one build, not across toolchains — a rendered key is a debugging address, not a value for source.
