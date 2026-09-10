# #191 — Drop Provider::get_token_factory

Merged 2026-08-27 into `master` from `refactor/drop-get-token-factory`, commit [`b167e23`](https://github.com/ulo-rs/ulo/commit/b167e238b07aafeea88bfac4c39f01e822e66f5b).

Removes `get_token_factory` from the `Provider` trait and keys the container's export-instance matching and role registry on `get_token`.

Every generated and hand-written implementation — the provider macros, core's built-ins, all nine integration crates — returned the same string for both methods, and the container's correctness rests on that equality: export resolution paired an instance's `get_token_factory` against the module's declared exports, while dependency resolution looked instances up by `get_token`. A second required method whose only correct implementation is "return `get_token()` again" invites divergence, and the GraphQL crates diverged: their service providers returned `"GraphQLServiceFactory"` while the module exports `"GraphQLService"`, so the declared export never matched a built instance and cross-module injection of the service failed with a permanently deferred resolution ("exports 'GraphQLService' but instance not yet created").

- **Core** — the trait loses the method; `instance_loader` and `container` key on `get_token`.
- **Macros** — the provider-variant and injectable emissions stop generating it.
- **Crates** — all 37 implementations across core, toni-config, the DB integrations, terminus, redis-broadcast, and both GraphQL crates are removed; the GraphQL divergence goes with them.
- **Tests** — `graphql_service_injection.rs` injects the exported service across the module boundary; it fails on the previous keying with the deferred-resolution error and passes here.

Breaking for hand-written `Provider` impls: delete the method.
