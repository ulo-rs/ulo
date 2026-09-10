# #110 — fix(config): gate config validation on the #[validate] attribute

Merged 2026-06-21 into `master` from `fix/config-validation`, commit [`c64ad6d`](https://github.com/ulo-rs/ulo/commit/c64ad6dca90932117cad62827df348895af6e6a2).

Config validation runs at load time, gated by the `#[validate(...)]` attribute itself. A field carrying `#[validate(...)]` is checked when the config loads; a config without those attributes never references the validator crate. Validation is opt-in per field, with no Cargo feature involved.

The derive previously wrapped the validator call in `#[cfg(feature = "validation")]`. That cfg resolves against the crate being compiled — the application deriving `Config` — rather than toni-config, so enabling `toni-config/validation` left `validate()` compiling to `Ok(())`: a config could load values that violated its own constraints and pass.

## Macro
- Emit the validator call whenever a `#[validate]` attribute is present, with no feature gate.

## toni-config
- Remove the `validation` feature and the optional `validator` dependency; core code never referenced validator. `validator` is a dev-dependency for the tests.
- Document attribute-gated, feature-free validation; drop the unused `pub use validator`.

## Tests
- Cover the reject (out-of-range), pass (in-range), and default-when-unset paths.

## Example
- `config_validation` shows a config rejected at startup, with a one-line trigger for the failure.

Removing the `validation` feature is technically breaking for anyone setting `features = ["validation"]`, though the feature never had any effect.
