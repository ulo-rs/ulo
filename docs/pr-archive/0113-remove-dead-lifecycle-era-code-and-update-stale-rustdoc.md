# #113 — Remove dead lifecycle-era code and update stale rustdoc

Merged 2026-07-14 into `master` from `docs/stale-rustdoc-cleanup`, commit [`cbbd632`](https://github.com/ulo-rs/ulo/commit/cbbd6326480a24054e98567efd291f06d9582de9).

Removes code and documentation stranded by the adapter lifecycle rework (the move from \`create(module, adapter)\` + \`app.listen()\` / trait-level \`close\` to \`use_http_adapter\` + \`start()\` and self-contained lifecycle handles) and by the \`HandlerContext\` refactor.

**Dead code**
- \`ServerHandle\` is deleted: it had no producer or consumer since adapters started returning lifecycle handles, only a definition, re-exports, and imports in four adapter crates.
- Unused \`Pin\`/\`Future\`/\`SocketAddr\`/trait imports removed across core, all five HTTP adapters, the RPC transports, tests, and examples. \`cargo check --workspace --all-targets\` is now free of unused-import warnings.

**Rustdoc**
- Crate headers in toni-axum, toni-actix, toni-tungstenite, toni-juniper, and toni-async-graphql now show the current bootstrap instead of \`create(module, adapter)\` + \`app.listen()\`.
- toni-salvo/toni-poem headers no longer reference \`HttpAdapter::close\` / a \`listen\` future; their panic sections now state the actual behavior (bind failures surface as \`Err\` from \`app.start()\`).
- \`AdapterContext\` links \`HttpAdapter::into_lifecycle\` instead of the removed \`HttpAdapter::create\`; the \`GrpcAdapter\` trait docs describe the \`bind\` → \`into_lifecycle\` flow instead of bind → serve → close.
- The \`set_metadata\` docstring shows \`Guard<HttpContext>\` + \`context.route_metadata()\` instead of the deleted \`Context\` + \`context.metadata().unwrap()\` pattern.
- All broken intra-doc links in toni repaired, several of which pointed at items that no longer exist (\`GrpcAdapter::local_addr\`, \`ClientsModule\`) or at wrong module paths (\`RpcError\`, \`WsError\`, \`Provider::execute\`, \`PanicRecovered\`). \`cargo doc\` is link-warning-free.

**Examples**
- \`pipes_complete_guide.rs\` claimed Toni has no global pipes and no \`@UsePipes()\` equivalent; \`use_global_http_pipes\` and \`#[use_pipes]\` both exist. The guide now states the actual design: \`Validated<T>\` in the handler signature is the idiomatic validation path, pipes serve cross-cutting transforms.
- \`route_metadata.rs\` and \`custom_extractors.rs\` headers show the current \`route_metadata()\` read pattern.

Pre-existing dead-code warnings inside toni-macros internals (never-used helper functions) are left for a separate cleanup.
