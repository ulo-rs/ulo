# #225 — Serve gRPC over TLS

Merged 2026-09-01 into `master` from `feat/grpc-tls`, commit [`60602a1`](https://github.com/ulo-rs/ulo/commit/60602a160ba7d1d7649ddb76054662b374ea72d6).

`Server::tls_config` must be applied before routes are added, and that happens inside `into_lifecycle`. So TLS is the one gRPC capability a caller cannot reach from outside the adapter.

```rust
let identity = Identity::from_pem(fs::read("server.pem")?, fs::read("server.key")?);
let adapter = GrpcAdapter::new(addr).with_tls(ServerTlsConfig::new().identity(identity));
```

`ServerTlsConfig` is taken as tonic defines it. Where certificates come from, how they rotate, and whether client certificates are demanded differ per deployment; mTLS is `client_ca_root` and the rest of the surface is tonic's. A wrapper of this framework's own would answer those questions on the deployment's behalf.

## Failing at bind, not in the serve loop

The acceptor is built during `app.bind()`, so a certificate the process cannot read refuses startup with the socket unopened — ADR-0024's refuse-or-none contract. `a_certificate_that_cannot_be_read_fails_bind` asserts on the rendered source chain, so it fails if the reason ever stops being the certificate rather than merely "grpc failed".

That required moving the server construction out of the serve future, since a builder has to exist before it can be validated. The layer stack is unchanged.

## The crypto provider is the caller's

`tls-ring` and `tls-aws-lc` mirror tonic's own feature names rather than picking one here. Enabling both is redundant rather than broken — tonic's provider match takes the ring arm first — so `--all-features` builds are unaffected.

## Tests

A client trusting a per-run self-signed certificate completes the handshake and the call; certificates are minted by `rcgen` at test time, so no fixture in this repo can expire.
