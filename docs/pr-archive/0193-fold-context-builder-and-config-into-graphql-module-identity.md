# #193 — Fold context builder and config into GraphQL module identity

Merged 2026-08-27 into `master` from `fix/graphql-module-identity`, commit [`517d441`](https://github.com/ulo-rs/ulo/commit/517d44116aabb0d9a9fe533971d716035a99ae51).

GraphQL module identity covered only the three schema types, so a second import differing in context builder, path, or playground setting shared the first one's id and was dropped by module dedup with nothing reported: a different context builder never ran, and a second endpoint for the same schema could not exist.

The id now folds in the context-builder type and a hash of the value config — the `DynamicModule` identity model (base plus fingerprint) — in both toni-async-graphql and toni-juniper:

- An identical import still dedups as a diamond.
- The same schema at two paths is two modules, and both mount — two GraphQL endpoints from one schema, previously impossible.
- A different context builder colliding on one path is refused by the native router's duplicate-route check instead of one builder winning by import order. The refusal is a panic out of `bind()` rather than a `StartupError`; that shape predates this change and is tracked separately.

The `graphql_module_identity` tests pin all three behaviors; the two-endpoints and refusal tests fail on master (the second path 404s, and no refusal occurs).

Known consequence: two mounted GraphQL modules both export the `"GraphQLService"` string token, so a module importing both resolves the first found — the same ambiguity as any two modules exporting one token.
