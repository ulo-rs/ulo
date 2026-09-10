# #8 — Call lifecycle hooks on non-singleton provider/controller instantiation

Merged 2026-03-22 into `master` from `feat/lifecycle-non-singleton`, commit [`bfe3f9a`](https://github.com/ulo-rs/ulo/commit/bfe3f9a6e109c69dfc2bdc96919616cb73706686).

Enable lifecycle hooks to be called for request- and transient-scoped providers and controllers during instantiation, aligning behavior with NestJS. This change ensures that `onModuleInit` and `onApplicationBootstrap` are invoked appropriately, while maintaining consistency by skipping shutdown hooks for non-singletons.
