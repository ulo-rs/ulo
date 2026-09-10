# #89 — docs(http): explain bounded-in-flight posture on the HttpAdapter trait

Merged 2026-05-30 into `master` from `docs/http-bounded-inflight-posture`, commit [`6128e22`](https://github.com/ulo-rs/ulo/commit/6128e22348ba041caa06713edcb4946511adb29b).

## Summary

A future contributor asking \"why no \`with_max_inflight\` on \`HttpAdapter\` like there is on \`RpcAdapter\` / \`GrpcAdapter\`?\" lands on the public trait doc and now gets the answer there.

The note frames the absence as posture, not omission: HTTP adapters wrap five framework crates (axum, actix, poem, rocket, salvo) each with their own middleware model, and the framework already owns a cross-adapter abstraction that solves bounded-in-flight without touching the trait surface — the global middleware chain in \`AdapterContext\` runs pre-routing on every HTTP adapter. Users wanting the cap can either add a semaphore-backed \`Middleware\` to the global chain (\`app.use_global_middleware(...)\`), reach for a framework-native answer (tower layer on axum/poem, actix-web middleware, Rocket fairing, etc.), or front the server with a reverse proxy.

Folding the knob into the trait would mean five framework-specific integrations for one feature the existing middleware path solves once.

## Tests

\`cargo doc --no-deps -p toni\` clean. No code change.
