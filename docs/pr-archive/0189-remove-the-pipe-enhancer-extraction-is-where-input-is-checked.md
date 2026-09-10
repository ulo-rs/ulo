# #189 — Remove the Pipe enhancer; extraction is where input is checked

Merged 2026-08-26 into `master` from `refactor/remove-pipes`, commit [`1f63c03`](https://github.com/ulo-rs/ulo/commit/1f63c0326094cffb0e622a42c96bc0cbae064da6).

`Pipe<C, R>` received the context, not a value, and ran between the interceptor chain and the handler — which is before extraction, so no handler argument existed for it to transform. The contexts hand out shared references, and it could not rewrite the input either. `HttpContext::take_request` takes `&self`, so the shape closest to a transform — a pipe reaching for the body — left every body extractor on that route failing with `BodyAlreadyRead`.

What remained was attaching to `ctx.extensions()` and continuing, which a `Guard` does asynchronously, or returning `Some(R)` to answer, which an `Interceptor` does asynchronously. Sync-ness was the only property the role held alone.

The seam it was named for is `FromContext`: an extractor is handed the raw input, produces the typed value the handler receives, and refuses by returning `Err`.

## Core

The trait, the per-transport entry and factory types, the `ProviderRole` variants, the registry slots, the container globals, `use_global_{http,rpc,ws}_pipes`, the `APP_PIPE` token, `#[use_pipes]`, and the three dispatcher loops are removed. `PipelineSegment::Pipe` goes with them; the enum is `#[non_exhaustive]`, so a downstream `match` already needed a wildcard arm.

The body-DTO seam goes in the same cut. `Validatable` has no implementor in the workspace, the controller macro emits `None` for every route, and the function that could build one is uncalled and returns `None`, so the 400 branch in `execute_handler` has never run. `Route::get_body_dto` and the two hand-written impls answering it go with it.

## Extraction

`Validated<E>` is no longer bound to `HttpContext`. It implements `FromContext<C>` for whatever context its inner extractor reads from, and `Payload<T>` joins the extractors it can wrap, so `Validated<Payload<T>>` validates a WebSocket frame and an RPC payload. The RPC handler macro treats an unrecognised parameter type as the call's payload, so `Validated` had to join its known-extractor list.

## Example and record

`examples/validation_complete_guide.rs` replaces the pipes guide, covering each job a pipe was reached for beside the declaration that does it, on all three transports. ADR-0027 carries the reasoning and the full mapping, and supersedes ADR-0016's treatment of `Pipe`.

## Behaviour a reviewer should weigh

Breaking, with no migration path: the framework is pre-release, so `#[use_pipes]` has no reader to warn.

An extraction failure on RPC or WebSocket renders as `Internal`, where a pipe could answer with any error it chose. Every `Payload<T>` parse failure already rendered this way; correcting the kind is its own change.

Test fixtures that rejected from a pipe reject from an interceptor instead. The three pipe-panic tests lose their subject; guard, interceptor, handler, error-handler and renderer panics keep theirs.

## Evidence

`cargo test -p toni --lib` 92 passed, `cargo test -p integration-tests` 399 passed, `cargo check --workspace --all-features --all-targets --locked` clean, and `cargo doc --workspace --all-features --no-deps` clean under `-D warnings`. `rpc_validated_payload_admits_valid_and_refuses_invalid` and its WebSocket twin fail when the `Validated` wrapper is dropped from the handler signature, which is what distinguishes them from serde's own parse checking.
