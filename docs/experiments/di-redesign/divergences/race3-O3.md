# race 3, area O3 (`ulo-otel-otlp`)

Files written: `crates/ulo-otel-otlp/Cargo.toml` (feature line for ureq changed), `src/lib.rs`,
`src/client.rs`. Nothing compiled; every API used was read in the registry sources
(`ureq-3.4.2`, `opentelemetry-otlp-0.33.1`, `opentelemetry-http-0.33.1`, `rustls-0.23.46`).

## Points the area resolves

**Which provider ureq's rustls client is given.** The workspace's `ring`, as
`rustls::crypto::ring::default_provider()` in an `Arc`, handed through
`TlsConfig::builder().unversioned_rustls_crypto_provider(..)`. ureq's `build_config` prefers that
setter over `CryptoProvider::get_default()` and over its own `ring_if_enabled`, so rustls's
resolution (the F422 panic with two linked providers and none installed) is never reached.

**Roots: the platform verifier, not webpki.** `Cargo.toml` takes ureq's `platform-verifier`
feature in place of the skeleton's `rustls-webpki-roots` (S-skel's log O3 allowed it).
`RootCerts::PlatformVerifier` matches R1 (sqlx native roots) and the NATS link, and reaches a
collector behind a private CA the host trusts. `rustls_platform_verifier::Verifier::new(provider)`
receives the same explicit provider, so it does not resolve one either. The cost: a container
image with no CA store (distroless static) fails https exports until it has one, and Android needs
the platform verifier's own JNI initialisation. Platform-verifier's tree has no tokio
(`rustls-native-certs` on unix, `security-framework` on apple); to be confirmed by the
coordinator's `cargo tree -i tokio` premise check. Reverting is one feature name in `Cargo.toml`
and `RootCerts::WebPki` in `client.rs`.

**The S7 refusal is unreachable.** The explicit provider can always be built (the workspace
`rustls` entry has `ring`), and ureq consults no process state once one is given, so the "more
than one provider" case cannot arise on this path. `UreqClient::new` keeps its `Result` and
refuses in the one case that remains: `ClientConfig::builder_with_provider(ring)
.with_protocol_versions(ALL_VERSIONS)` failing, which ureq would otherwise raise as a panic
(`expect("all TLS versions")`) on the first https connection. Its message is
"the ring crypto provider cannot serve a TLS client: {error}", not S7's text, because S7's text
would misdescribe that condition. The brief said to refuse "with the message S7 ... if it cannot
be built that way"; it can be, so S7's message is not used (divergence, deliberate).

## Divergences

1. **`reqwest-blocking` compiles `client.rs` out** (`#[cfg(not(feature = "reqwest-blocking"))] mod
   client`) and `build` then sets no `with_http_client`, leaving the exporter crate's own default,
   reqwest's blocking client. `ureq`, `rustls`, `async-trait`, `bytes` stay in the manifest
   as non-optional dependencies, so the feature adds reqwest beside ureq rather than replacing it.
   Making ureq optional would reshape the features table (frozen by the skeleton); not done.
2. **The endpoint gets `/v1/traces` appended.** `OtelOptions::endpoint` is documented as
   `OTEL_EXPORTER_OTLP_ENDPOINT`, a base URL; `opentelemetry-otlp`'s `with_endpoint` treats its
   argument as the whole URL. `lib.rs` appends the path (trailing slashes trimmed). With no
   endpoint nothing is set and the exporter crate reads `OTEL_EXPORTER_OTLP_*` itself and defaults
   to `http://localhost:4318/v1/traces`, which is a read of the process environment outside the
   `Env` `OtelOptions::from_env` goes through. O2 may want to know.
3. **The request timeout is a fixed ten seconds** (`TIMEOUT`), passed to both `with_timeout` and
   `UreqClient::new`. `OtelOptions` has no timeout field, and with a custom client the exporter
   crate's `OTEL_EXPORTER_OTLP_TIMEOUT` handling would not reach ureq anyway. The value is the
   exporter crate's own default.
4. **Headers are parsed and validated here** (`parse_headers`): `key=value,key=value`, entries
   trimmed, empty entries skipped, an entry without `=` or with an invalid header name or value
   refused at `build` naming its position and never its text (the field is a `Secret`).
   `opentelemetry-otlp` drops invalid entries silently. Values stay percent-encoded;
   `with_headers` decodes them. Headers from `OTEL_EXPORTER_OTLP_HEADERS` in the environment are
   still merged in by the exporter crate and win over these.
5. **Protocol set explicitly** to `Protocol::HttpBinary`, so `OTEL_EXPORTER_OTLP_PROTOCOL` cannot
   select a variant the feature set lacks.
6. **Client body limit.** Response bodies are read to at most 4 MiB; beyond that the client
   returns `ResponseBodyTooLarge`, as the crate's built-in clients do. The request is rebuilt on
   `ureq::http` rather than passed through, so the two `http` versions need not be one crate.
   Redirects and proxy-from-environment are ureq's defaults.

## Requests

None. (`Cargo.lock` will change: `rustls-platform-verifier` is already in the lock file, via
another crate, at 0.7.x; `webpki-roots` is no longer needed by this crate.)

## Not verified

- That `ureq`'s `platform-verifier` feature resolves `rustls-platform-verifier` to a version
  whose `rustls` is the workspace's single 0.23.x (the registry holds 0.7.0 and 0.7.1; the lock
  decides).
- That the `Arc<CryptoProvider>` type ureq names is the workspace's `rustls` (one 0.23 in the
  lock); a split would be a type error at `unversioned_rustls_crypto_provider`.
- Runtime behaviour: nothing was run. The coordinator's P8 premise check covers the explicit
  provider.

## Replaced on `master`

Nothing: `ulo-otel-otlp` did not exist there.
