# #76 — refactor(core): collapse Controller's enhancer acces

Merged 2026-05-06 into `master` from `cleanup/controller-enhancers-accessor`, commit [`4b93553`](https://github.com/ulo-rs/ulo/commit/4b93553d9ffb93b71b0b3ff8e17ee78557370d37).

Eight accessors all defaulting to `vec![]` and all consumed once by
`resolve_enhancers_from_tokens` pretended that DI tokens and
direct-instantiation arcs were independent capabilities. They aren't:
they're one logical concept (this route's enhancer manifest) in two
storage forms. One typed accessor returning `ControllerEnhancers` reads
honestly and matches the way the resolver actually consumes the data.
