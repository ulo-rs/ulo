# #126 — fix: key modules by identity and add named database connections

Merged 2026-07-18 into `master` from `fix/module-identity`, commit [`0ae328a`](https://github.com/ulo-rs/ulo/commit/0ae328a1174da795427d1bd9c4063499badad767).

Modules were identified by a hardcoded string and registered last-wins, so two `SeaOrmModule::for_root(a)` and `for_root(b)` silently overwrote each other — the application ran with one connection and no error. This keys modules by content so duplicates are caught, and adds a named-connection constructor for the cases where a second connection of the same type is genuinely wanted.

## Injector

- Static modules key by `type_name` (collision-free) rather than the bare identifier.
- Dynamic modules fold a configuration fingerprint into their identity via the new `ProviderFactory::identity_hint` hook. Identical registrations dedup (a module reached through two import paths); different configs stay distinct.
- Two distinct global modules exporting the same provider token are refused at startup instead of one silently shadowing the other.
- `add_module` returns `Result`; the scan dedups on identity.

## Database integrations

- Each integration exposes a named constructor — `for_root_named`, or `<backend>_named` for the crates that split by backend (sqlx, diesel) — registering the connection under a caller-chosen name, injected with `#[inject("<name>")]`. The default constructor is unchanged and stays injectable by type.
- Connection factories implement `identity_hint` so that omitting the name on a second connection fails loudly rather than silently. A name is what actually creates a second connection in every crate; the fingerprint only decides whether the omission is caught. Prisma cannot participate — its connect closure is opaque, so two unnamed prisma clients dedup silently — and its docs direct users to the named constructor.

## Behavior changes

- Two unnamed `for_root` connections now abort startup with a message naming both modules, where before one silently survived.
- `ModuleRef::current_module()` returns the fully-qualified module identity (its resolution key) rather than the short name.

## Tests

Six in-crate tests drive the real scanner and instance loader with a configuration-only fake provider: identity dedup, coexistence of different configs, the export clash, and named resolution. In-crate because the scanner and loader are not public API.

Deferred: the type-marker form of multiplicity (`SeaOrmModule::<Primary>`, inject-by-type instead of by-name). The named constructor covers the need and the marker form adds boilerplate in user code; the `type_name` keying here is its groundwork if it is ever wanted.
