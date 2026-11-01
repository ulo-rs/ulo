//! Spelling probe for the gRPC pass, run against master `654c7183` on 2026-09-23.
//! Question: does `#[grpc_methods]` read a return type by name? It does, twice.
//!
//! **This file does not compile, by design.** It is the evidence for F240. Drop it
//! into `integration-tests/tests/integration/`, add `mod grpc_spelling_probe;` to
//! that directory's `main.rs`, then `cargo check -p integration-tests --tests`.
//!
//! Expected, both E0053 against the generated trait:
//!
//! - `create` returns `Answer<CreateOrderResponse>`, an alias for `Result<_, NotServed>`.
//!   `answer_type` matches the last path segment against `Result`, does not see one, reads
//!   the handler as infallible, and emits `tonic::Response<Result<CreateOrderResponse, NotServed>>`.
//! - `bulk_create` returns `Result<Reply<BulkCreateResponse>, NotServed>`, where `Reply` aliases
//!   `tonic::Response`. `response_inner_type` matches against `Response`, does not see one, and
//!   emits `Response<Response<BulkCreateResponse>>`.
//!
//! Both diagnostics point at the `#[grpc_methods]` attribute and name neither the alias nor
//! the rule. On HTTP, RPC and WebSocket the same spellings work: ADR-0050's blanket
//! `impl<T: Transport, V: IntoOutput<T>, E: Into<T::Error>> IntoOutput<T> for Result<V, E>`
//! resolves through an alias, and nothing there reads a name.

#![allow(dead_code)]

use futures_util::Stream;
use ulo::extract::Payload;
use ulo::grpc::extract::Inbound;
use ulo_macros::{controller, grpc_methods, new};

use crate::common::NotServed;

mod spell_pb {
    tonic::include_proto!("ulo_test.orders");
}

use spell_pb::orders_server::Orders;

/// The same type, spelled so the last path segment is not `Result`.
type Answer<T> = std::result::Result<T, NotServed>;
/// The same type, spelled so the last path segment is not `Response`.
type Reply<T> = tonic::Response<T>;

#[controller]
pub struct SpellingService {}

impl SpellingService {
    #[new]
    pub fn new() -> Self {
        Self {}
    }
}

#[grpc_methods(spell_pb::orders_server::Orders)]
impl SpellingService {
    /// PROBE A — an alias for `Result`. On HTTP/RPC/WS this is ADR-0050's own
    /// case: the blanket impl puts the error on the error side whatever the
    /// alias is called.
    #[grpc_method]
    async fn create(
        &self,
        Payload(_req): Payload<spell_pb::CreateOrderRequest>,
    ) -> Result<spell_pb::CreateOrderResponse, NotServed> {
        Ok(spell_pb::CreateOrderResponse {
            id: 1,
            status: "ok".to_string(),
        })
    }

    #[grpc_stream]
    async fn watch_progress(
        &self,
        Payload(_req): Payload<spell_pb::WatchRequest>,
    ) -> Result<
        impl Stream<Item = Result<spell_pb::ProgressEvent, NotServed>> + Send + 'static,
        NotServed,
    > {
        Ok(futures_util::stream::empty())
    }

    #[grpc_method]
    async fn bulk_create(
        &self,
        _inbound: Inbound<spell_pb::CreateOrderRequest>,
    ) -> Result<Reply<spell_pb::BulkCreateResponse>, NotServed> {
        Err(NotServed)
    }

    #[grpc_stream]
    async fn chat(
        &self,
        _inbound: Inbound<spell_pb::ChatMessage>,
    ) -> Result<
        impl Stream<Item = Result<spell_pb::ChatMessage, NotServed>> + Send + 'static,
        NotServed,
    > {
        Ok(futures_util::stream::empty())
    }
}
