# ulo-http-conformance

The conformance suite every `ulo-http` host runs. A host is the HTTP server the app runs inside:
axum, actix-web, poem, rocket, salvo, or one written outside the `ulo` repository, mounted
through `ulo_http::embed`. One macro stamps the suite's scenarios as `#[test]`s against the host,
each in two modes, the app nested under a prefix and as the host's fallback: routing and its
404, 405, `OPTIONS` and `HEAD` answers, extraction failures, the wire's edge cases (two
`Set-Cookie` lines, a refused `\r\n` in a header, a missing or doubled `Host`), reshaped and
recovered errors, a pre-dispatch entry answering a preflight, server-sent events delivered as
produced, a client that disconnects mid-stream or before the head, an upgrade, values the host
passes to the app, and a drain on HTTP/1.1 and HTTP/2. Wherever no declared limit applies, an answer is compared byte for
byte with the hyper backend's on the same runtime. A host whose `tests/conformance.rs` passes keeps the same
promises as the hosts `ulo` ships, each of which runs the suite in CI.

## Install

The suite is a dev-dependency of the host crate. The example below runs it against the hyper
backend, the reference host, which stands in for yours:

```toml
[dev-dependencies]
ulo-http-conformance = { version = "=0.1.0-alpha.3", features = ["tokio"] }
```

The crate is pinned with `=`, since a caret on a pre-release also takes a later alpha, which may
break this one. `tokio` brings `OnTokio`, the runtime half of a host whose framework runs on
tokio; a host on smol enables `smol` and names `OnSmol` instead. A rocket host also enables
`rocket`, for the fairing that reports the app's `Routing`.

## First run

`tests/conformance.rs`:

```rust
use ulo_http_conformance::{HyperHost, OnTokio, http_conformance_suite};

http_conformance_suite!(HyperHost<OnTokio>; not_applicable {
    routing_extension: "the reference has no host around the app to read `Routing`",
});
```

`cargo test --test conformance` runs every scenario as a test of its own, named after the
scenario, in a `nested` and a `fallback` module, and reports the declared one as ignored with its
reason.

Your host takes `HyperHost<OnTokio>`'s place, as a type implementing `Host`:

- `start(app, mode)` binds the suite's app, connected and not yet bound, into the host: nested,
  `Embedded::<YourHost>::new().nested_at(PREFIX)`, and as the fallback `Embedded::new()`. It runs
  `listen()`, serves the host on a port the OS chooses and wires the host's graceful shutdown to
  `Handle::stopping`.
- `base_url` answers where the host serves, `limits` the `EmbedLimits` its `Embed` declares,
  `connections_read` how many connections the host's server has read from (a `ReadCount` over
  what its listener accepts, or `None`), and `stop` shuts the app and the host down.
- A host middleware writes the app's `Routing` onto the response as `ROUTING_HEADER`, spelled by
  `routing_label`, and copies the `HOST_VALUE_HEADER` request header into the host's own request
  store as a `HostValue`, or the host declares it has none and `Embedded::forward` copies it.

[`community-host`](https://github.com/ulo-rs/ulo/tree/master/crates/ulo-codegen-tests/fixtures/community-host)
is a complete host written as a crate outside the workspace would write it, a hyper server with
its own `Embed` implementation, against the public API of `ulo-http` alone, with its
`tests/conformance.rs` run in CI.

## What a host declares

A host's `EmbedLimits` hold every ability unless the host declares one absent with
`without_<ability>(reason)`. A host that declares nothing is held to every scenario. The limits
are checked in both directions: a host that passes a scenario it declares unsupported fails, and
one declaring `forward_miss` that answers a `Forwardable` 404 itself fails the same way.

Five more declarations record what a host does where hosts differ, and refuse nothing:
`request_body`, `disconnect`, `drain_pending`, `drain_abandoned` and `drain_http2`, each set
through its own setter on `EmbedLimits::NONE`. The scenarios on body limits, disconnects and the
drain read them, and hold the host to the value declared.

`not_applicable { scenario: "reason" }` is for a scenario no capability describes, such as a wire
check the host's server makes before the app sees the request. The test is stamped
`#[ignore = "<reason>"]`, a name that is no scenario fails to compile, and
`cargo test -- --ignored` runs it, where it fails.

## Scenarios

The scenarios are the functions of the [`cases`](https://docs.rs/ulo-http-conformance/latest/ulo_http_conformance/cases/)
modules, each generic over the host and taking the mode, so a host crate runs one alone to debug it.

## Will it work for me

- **Rust 1.88** or later.
- **Runtimes:** the suite names none. `tokio` and `smol` bring the two harnesses; another runtime
  implements `Harness`. Every wait a scenario makes is bounded by the app's `Timer`.
- **Parallelism:** the scenarios run in parallel under libtest. A host short of memory sets
  `ULO_CONFORMANCE_PARALLEL` to a positive integer to bound how many run at once.
- **Startup failures** are written to a file before the test panics: under `$ULO_CONFORMANCE_FAILURES`
  when it is set, otherwise under `conformance-failures` in the target directory the test binary
  was built into.
- **Stability:** the scenarios, `Host`, `Harness` and `http_conformance_suite!` are the contract a
  host is held to. While the version is a pre-release, every `ulo` crate releases at one version
  and a pre-release may change any of them. From `0.1.0` on, a release that adds or tightens a
  scenario or changes the contract is a minor release, `0.1` to `0.2`, since a host that passed
  the earlier suite can fail it.

## Where to go next

- The API docs on [docs.rs](https://docs.rs/ulo-http-conformance), or `cargo doc --open`.
- The suites of the hosts `ulo` ships:
  [axum](https://github.com/ulo-rs/ulo/blob/master/crates/ulo-http-axum/tests/conformance.rs),
  [actix-web](https://github.com/ulo-rs/ulo/blob/master/crates/ulo-http-actix/tests/conformance.rs),
  [poem](https://github.com/ulo-rs/ulo/blob/master/crates/ulo-http-poem/tests/conformance.rs),
  [rocket](https://github.com/ulo-rs/ulo/blob/master/crates/ulo-http-rocket/tests/conformance.rs)
  and [salvo](https://github.com/ulo-rs/ulo/blob/master/crates/ulo-http-salvo/tests/conformance.rs),
  and the reference host's, [hyper](https://github.com/ulo-rs/ulo/blob/master/crates/ulo-http-hyper/tests/conformance.rs).

## License

MIT.
