# #26 — feat: inject multiple providers as a Vec

Merged 2026-04-01 into `master` from `feat/multi-providers`, commit [`a1c3e90`](https://github.com/ulo-rs/ulo/commit/a1c3e90e96a4ccc70d7cdb7180af3e6af8058cad).

NestJS's multi: true pattern — multiple providers contributing to the
same injection token, collected into a Vec at the injection site. This
fills a major DI gap: plugin systems, strategy registries, and
middleware chains all need to accumulate implementations without any
single module knowing the full set.

## Usage

```rust
provide!("PLUGINS", PluginA, multi(Plugin))
provide!("PLUGINS", || PluginA::new(), multi(Plugin))
provide!("PLUGINS", existing(PluginA), multi(Plugin))
// ...all provide! variants supported

#[inject("PLUGINS")]
plugins: Vec<Arc<dyn Plugin>>,
