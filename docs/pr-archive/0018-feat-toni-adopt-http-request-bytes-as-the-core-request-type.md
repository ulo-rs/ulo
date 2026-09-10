# #18 — feat(toni): adopt http::Request<Bytes> as the core request type

Merged 2026-03-27 into `master` from `feat/http-crate-adoption`, commit [`707c72b`](https://github.com/ulo-rs/ulo/commit/707c72b82808a759539a6d0f154b7eb5a3520c04).

## Summary

- `HttpRequest` is now a thin newtype over `http::Request<Bytes>` instead of a custom representation, eliminating ~500 lines of bespoke request machinery
- `HttpRequest::builder()` proxies `http::request::Builder` so callers get standard construction without a direct `http` crate dependency
- Tower compatibility bridge types (`ToniPathParams`, `ToniExtensionBridge`, `toni_extensions()`) are removed — Tower layers now see `http::Request<Bytes>` natively, which is what the Tower ecosystem expects

## Motivation

Building a custom request type that mirrors what `http` already solves creates maintenance debt and eventual incompatibility. This is groundwork for the upcoming streaming body branch: adopting the standard type now means that branch starts from a solid, interoperable foundation rather than a custom one with the same walls.
