use std::collections::HashMap;

use async_trait::async_trait;

use crate::rpc::{RpcClientError, RpcData, RpcReplyStream};

/// Interface for RPC client transports.
///
/// Transport crates implement this trait to send messages to remote services.
/// `RpcClient` wraps any `RpcClientTransport` and provides the user-facing API.
///
/// - [`send`] — request-response: waits for a reply
/// - [`emit`] — fire-and-forget: returns once the message is dispatched
///
/// Both carry per-call `metadata` — a flat string map the transport places on
/// its native side channel (NATS/AMQP/MQTT headers, or the request envelope for
/// Redis/TCP/UDP) and the server surfaces as `RpcContext` metadata. The map is
/// empty for the plain `RpcClient::send`/`emit` shorthands.
///
/// [`send`]: RpcClientTransport::send
/// [`emit`]: RpcClientTransport::emit
#[async_trait]
pub trait RpcClientTransport: Send + Sync + 'static {
    /// Open the connection ahead of the first call.
    ///
    /// [`RpcClient::connect`](crate::rpc::RpcClient::connect) forwards to it, and
    /// the framework calls that at application bootstrap for a client the
    /// container holds as its own [`Provider`](crate::spi::Provider); a failure
    /// this reports then surfaces at startup rather than on the first call. For
    /// any other client nothing calls it until the caller does. The default does
    /// nothing, and a transport that opens its connection on demand overrides
    /// it to open that connection here. Whether an unreachable peer is reported
    /// here, on the first call, or not at all is that transport's.
    async fn connect(&self) -> Result<(), RpcClientError> {
        Ok(())
    }

    /// The transport's shutdown step.
    ///
    /// Called by [`RpcClient::close`], which the framework calls at application
    /// shutdown for a client the container holds as its own
    /// [`Provider`](crate::spi::Provider). The default does nothing. An override
    /// is where a transport flushes what it buffers; one that buffers and keeps
    /// the default loses what is still queued when the process exits.
    ///
    /// [`RpcClient::close`]: crate::rpc::RpcClient::close
    async fn close(&self) -> Result<(), RpcClientError> {
        Ok(())
    }

    /// Send a message and wait for the remote reply (request-response).
    async fn send(
        &self,
        pattern: &str,
        data: RpcData,
        metadata: HashMap<String, String>,
    ) -> Result<RpcData, RpcClientError>;

    /// Send a message without waiting for a reply (fire-and-forget).
    async fn emit(
        &self,
        pattern: &str,
        data: RpcData,
        metadata: HashMap<String, String>,
    ) -> Result<(), RpcClientError>;

    /// Open a streaming call: one request, many reply frames until the end
    /// marker (ADR-0032).
    ///
    /// Implementations feed items into an [`RpcReplyStream`], enforce their
    /// `with_timeout` as the per-frame gap (the first frame included), and
    /// send the cancel notice from the stream's `on_cancel` when the caller
    /// drops it early. The default answers
    /// [`RpcClientError::StreamingUnsupported`], so a transport predating the
    /// grammar keeps compiling — and refuses loudly.
    async fn open_stream(
        &self,
        pattern: &str,
        data: RpcData,
        metadata: HashMap<String, String>,
    ) -> Result<RpcReplyStream, RpcClientError> {
        let _ = (pattern, data, metadata);
        Err(RpcClientError::StreamingUnsupported)
    }
}
