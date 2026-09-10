# #39 — fix: Carry dependency roles through build for alias providers

Merged 2026-04-18 into `master` from `fix/alias-provider-role-forwarding`, commit [`a805521`](https://github.com/ulo-rs/ulo/commit/a805521591ee9f0e4d5083375cd0434438f45289).

Enhance the build process to carry dependency roles alongside provider instances, allowing alias providers to forward roles correctly. This change addresses issues with role registration when using alias tokens in guards.
