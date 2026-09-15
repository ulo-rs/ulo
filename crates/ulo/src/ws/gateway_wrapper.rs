use crate::dispatch::transport::Ws;
use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::RwLock;

use crate::context::Metadata;
use crate::dispatch::ExecutionResult;
use crate::enhancer::{Guard, Interceptor, InterceptorNext};
use crate::errors::{PanicRecovered, PipelineSegment};
use crate::spi::{WsErrorHandlerArc, WsGuardEntry, WsInterceptorEntry};
use crate::ws::WsContext;

use super::{
    DisconnectReason, Gateway, WsClient, WsError, WsHandlerOutput, WsHandlerResult, WsMessage,
};
use futures::stream::BoxStream;
use futures::{FutureExt, StreamExt};
use std::panic::AssertUnwindSafe;

/// Delegates to an inner stream while holding something alive alongside it.
///
/// `BoxStream` is a `Pin<Box<_>>` and therefore `Unpin`, so the projection needs
/// no pin machinery. The HTTP side does the same for response bodies.
struct ScopedStream {
    inner: BoxStream<'static, WsMessage>,
    context: WsContext,
    /// Set once the inner stream answers `None`, which is the end of it.
    drained: bool,
}

impl futures::Stream for ScopedStream {
    type Item = WsMessage;

    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        let this = self.get_mut();
        let polled = std::pin::Pin::new(&mut this.inner).poll_next(cx);
        if matches!(polled, std::task::Poll::Ready(None)) {
            this.drained = true;
        }
        polled
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

/// A stream dropped with messages still to come is the connection having gone. The handler returned
/// when it had a stream, so whatever feeds that stream is not inside a future anything drops.
impl Drop for ScopedStream {
    fn drop(&mut self) {
        if !self.drained {
            use crate::context::ExecutionContext as _;
            self.context.cancellation().cancel();
        }
    }
}

struct WsChainNext {
    interceptors: Vec<Arc<dyn Interceptor<WsContext, WsHandlerResult>>>,
    gateway: Arc<Box<dyn Gateway>>,
}

#[async_trait]
impl InterceptorNext<WsContext, WsHandlerResult> for WsChainNext {
    async fn run(self: Box<Self>, context: &WsContext) -> WsHandlerResult {
        GatewayWrapper::execute_with_interceptors(context, &self.interceptors, &self.gateway).await
    }
}

/// Parallel to `RoutePipeline` on the HTTP side — wraps a gateway with the full
/// guard/interceptor pipeline and tracks its own connected clients.
pub(crate) struct GatewayWrapper {
    gateway: Arc<Box<dyn Gateway>>,
    guards: Vec<WsGuardEntry>,
    interceptors: Vec<WsInterceptorEntry>,
    error_handlers: Vec<WsErrorHandlerArc>,
    metadata: Arc<Metadata>,
    /// Per-event metadata, already merged over `metadata` at expansion. An event absent here
    /// declared nothing of its own and reads the gateway's.
    handler_metadata: HashMap<String, Arc<Metadata>>,
    handler_guards: HashMap<String, Vec<WsGuardEntry>>,
    handler_interceptors: HashMap<String, Vec<WsInterceptorEntry>>,
    handler_error_handlers: HashMap<String, Vec<WsErrorHandlerArc>>,
    /// Active client connections (client_id => WsClient). A client carries the session scoped to
    /// its connection, so there is nothing to keep beside it.
    clients: Arc<RwLock<HashMap<String, WsClient>>>,
}

impl GatewayWrapper {
    pub(crate) fn new(
        gateway: Arc<Box<dyn Gateway>>,
        guards: Vec<WsGuardEntry>,
        interceptors: Vec<WsInterceptorEntry>,
        error_handlers: Vec<WsErrorHandlerArc>,
        metadata: Arc<Metadata>,
        handler_metadata: HashMap<String, Arc<Metadata>>,
        handler_guards: HashMap<String, Vec<WsGuardEntry>>,
        handler_interceptors: HashMap<String, Vec<WsInterceptorEntry>>,
        handler_error_handlers: HashMap<String, Vec<WsErrorHandlerArc>>,
    ) -> Self {
        Self {
            gateway,
            guards,
            interceptors,
            error_handlers,
            metadata,
            handler_metadata,
            handler_guards,
            handler_interceptors,
            handler_error_handlers,
            clients: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Phase 1 of connection setup: run guards and store client.
    ///
    /// The upgrade's parts are no longer threaded in: a connect guard reads the
    /// handshake through `ctx.client().handshake`, which is the same information
    /// by a route that works per-message too.
    /// Phase 1 of connection setup: run the guards that admit the connection.
    ///
    /// Returns the connect execution's context, which phase 2 finishes. The two phases exist so the
    /// adapter can register the client's sink between them, not because they are separate
    /// executions — a guard's writes have to reach the hook, so they share one context and one bag.
    ///
    /// Guards run here and interceptors do not, because a connect is an admission decision rather
    /// than a call. A guard answers admission, which is its whole contract. An interceptor wraps a
    /// call and its answer, and an error handler shapes an answer; a connection has none to wrap or
    /// shape, and refusing one is answered by not opening it. The same rule decides the message
    /// path: a refused message has an open socket to answer on, so it goes through the chain and
    /// the caller is told.
    pub(crate) async fn begin_connect(&self, client: WsClient) -> Result<WsContext, WsError> {
        // The client was born with its session, so a guard below writes to the store every later
        // execution on this connection reads.
        let context = WsContext::new(
            client.clone(),
            WsMessage::text(""),
            "connect",
            Some(self.metadata.clone()),
        );

        let guards = crate::enhancer::pipeline::guards_for::<Ws>(&self.guards, &context).await;
        for (i, guard) in guards.iter().enumerate() {
            // A panic in `can_activate` is treated as a hard rejection so the
            // dispatcher doesn't tear down: the panic is logged and the
            // connection is refused. A connect has no chain to route it
            // through — there is no answer to shape on a refused upgrade.
            let activated = match crate::panic_recovery::catch_async(
                crate::errors::PipelineSegment::Guard,
                guard.can_activate(&context),
            )
            .await
            {
                Ok(b) => b,
                Err(event) => {
                    // The refusal reaches the caller as a close frame, so the
                    // panic is narrated rather than reported. Keeping the event
                    // typed is what puts an internal-error close code on the
                    // wire instead of a policy one.
                    tracing::debug!(client_id = %client.id, guard_index = i, panic = %event.message, "connect guard panicked");
                    return Err(WsError::from(event));
                }
            };
            if !activated {
                tracing::debug!(client_id = %client.id, guard_index = i, "guard rejected WebSocket connection");
                return Err(WsError::AuthFailed("Guard rejected connection".into()));
            }
        }

        tracing::debug!(client_id = %client.id, "WebSocket client connected");
        self.clients.write().insert(client.id.clone(), client);
        Ok(context)
    }

    /// Phase 2 of connection setup: fire the `on_connect` lifecycle hook on the context phase 1
    /// built, so the hook reads the bag the guards wrote to.
    pub(crate) async fn complete_connect(&self, context: &WsContext) -> Result<(), WsError> {
        let client = context.client();
        if !self.clients.read().contains_key(&client.id) {
            return Err(WsError::ConnectionClosed("Client not found".into()));
        }

        self.gateway.on_connect(client, context).await
    }

    /// Handle one inbound frame, from routing it to framing the answer.
    ///
    /// Answers `Err` only when there is nothing left to answer on — the client has gone. Every
    /// other outcome, failures included, leaves as an `Ok` the caller writes to the socket.
    pub(crate) async fn handle_message(
        &self,
        client_id: String,
        message: WsMessage,
    ) -> Result<WsHandlerOutput, WsError> {
        let client = self
            .clients
            .read()
            .get(&client_id)
            .cloned()
            .ok_or_else(|| WsError::ConnectionClosed("Client not found".into()))?;

        // A control frame is the protocol talking, not the application. A keepalive names no
        // event because it is not asking for one, and answering it would put an error frame on
        // the wire in reply to a ping.
        if matches!(
            message,
            WsMessage::Ping(_) | WsMessage::Pong(_) | WsMessage::Close(_)
        ) {
            return Ok(WsHandlerOutput::Empty);
        }

        // A frame naming no event fails to route with the socket still open, so the caller is
        // told, exactly as it is for an event nothing handles. Guards and interceptors do not
        // run: a guard answers whether a caller may make some call and an interceptor wraps
        // that call, and this frame named none. An error handler does run, because there is an
        // answer to shape. Its context carries an empty event, for the same reason.
        let (event, unroutable) = match self.extract_event(&message) {
            Ok(event) => (event, None),
            Err(e) => {
                tracing::debug!(client_id = %client_id, reason = %e, "WebSocket frame did not route");
                (String::new(), Some(e))
            }
        };

        tracing::trace!(client_id = %client_id, event = %event, "WebSocket message received");

        let context = WsContext::new(
            client.clone(),
            message.clone(),
            event.clone(),
            Some(
                self.handler_metadata
                    .get(&event)
                    .unwrap_or(&self.metadata)
                    .clone(),
            ),
        );

        let mut all_error_handlers = self.error_handlers.clone();
        if let Some(h) = self.handler_error_handlers.get(&event) {
            all_error_handlers.extend_from_slice(h);
        }

        let answer = match unroutable {
            // Resolving guards and interceptors is what constructs them, execution-scoped ones
            // included, so an unroutable frame must not reach it — a flood of garbage would
            // otherwise build a pipeline per frame and run none of it.
            Some(e) => Err(e),
            None => {
                let mut all_guards = self.guards.clone();
                if let Some(h) = self.handler_guards.get(&event) {
                    all_guards.extend_from_slice(h);
                }
                let mut all_interceptors = self.interceptors.clone();
                if let Some(h) = self.handler_interceptors.get(&event) {
                    all_interceptors.extend_from_slice(h);
                }

                let guards =
                    crate::enhancer::pipeline::guards_for::<Ws>(&all_guards, &context).await;
                let interceptors =
                    crate::enhancer::pipeline::interceptors_for::<Ws>(&all_interceptors, &context)
                        .await;
                Self::run_chain(&context, &self.gateway, &guards, &interceptors).await
            }
        };

        // The one place the chain runs. A guard's refusal, a panic from any segment and the
        // handler's own error all arrive as `Err`, so a `#[catch]` handler is offered every one of
        // them and an unclaimed one renders the same envelope whichever produced it.
        //
        // A refused message renders rather than failing the call: the socket stays open and the
        // client learns its message went nowhere, which is what the read loop needs.
        let answer = match answer {
            Ok(output) => Ok(output),
            Err(ws_err) => {
                let observed: &(dyn std::error::Error + Send + Sync + 'static) = match &ws_err {
                    WsError::AppError(e) => e.as_ref(),
                    other => other,
                };
                match crate::enhancer::pipeline::claim::<Ws>(
                    &all_error_handlers,
                    observed,
                    &context,
                )
                .await
                {
                    Some(Ok(output)) => Ok(output),
                    Some(Err(reshaped)) => Ok(WsHandlerOutput::Single(Self::safe_render(|| {
                        reshaped.to_message()
                    }))),
                    None => Ok(WsHandlerOutput::Single(Self::safe_render(|| {
                        ws_err.to_message()
                    }))),
                }
            }
        };

        // The execution ends when the answer does. A stream has emitted nothing
        // at this point, so the context rides it rather than dying here.
        match answer {
            Ok(WsHandlerOutput::Stream(stream)) => Ok(WsHandlerOutput::Stream(
                ScopedStream {
                    inner: stream,
                    context,
                    drained: false,
                }
                .boxed(),
            )),
            other => other,
        }
    }

    /// Guards, then the interceptor chain. Every way this can fail leaves as `Err`.
    async fn run_chain(
        context: &WsContext,
        gateway: &Arc<Box<dyn Gateway>>,
        guards: &[Arc<dyn Guard<WsContext>>],
        interceptors: &[Arc<dyn Interceptor<WsContext, WsHandlerResult>>],
    ) -> WsHandlerResult {
        for (guard_index, guard) in guards.iter().enumerate() {
            // A guard's panic is a developer error, not a verdict: it takes the same route as any
            // other pipeline panic, so the chain sees `PanicRecovered` where a refusal gives it
            // `GuardRejection`.
            match crate::panic_recovery::catch_async(
                crate::errors::PipelineSegment::Guard,
                guard.can_activate(context),
            )
            .await
            {
                Ok(true) => {}
                Ok(false) => {
                    tracing::debug!(guard_index = guard_index, "guard rejected message");
                    return Err(WsError::from(crate::errors::GuardRejection::new(
                        guard_index,
                    )));
                }
                Err(event) => {
                    tracing::debug!(guard_index = guard_index, panic = %event.message, "guard panicked");
                    return Err(WsError::from(event));
                }
            }
        }

        Self::execute_with_interceptors(context, interceptors, gateway).await
    }

    async fn execute_with_interceptors(
        context: &WsContext,
        interceptors: &[Arc<dyn Interceptor<WsContext, WsHandlerResult>>],
        gateway: &Arc<Box<dyn Gateway>>,
    ) -> WsHandlerResult {
        if interceptors.is_empty() {
            return Self::execute_handler(context, gateway).await;
        }

        let (first, rest) = interceptors.split_first().unwrap();

        let next = WsChainNext {
            interceptors: rest.to_vec(),
            gateway: gateway.clone(),
        };

        match crate::panic_recovery::catch_async(
            crate::errors::PipelineSegment::Middleware,
            first.intercept(context, Box::new(next)),
        )
        .await
        {
            Ok(answer) => answer,
            Err(event) => Err(WsError::from(event)),
        }
    }

    /// Drive `WsError::to_message` with panic recovery — a panic in the
    /// renderer would close the connection without ever framing an outbound
    /// error message. Policy: log the panic and substitute a hardcoded text
    /// frame.
    fn safe_render<F>(render: F) -> WsMessage
    where
        F: FnOnce() -> WsMessage,
    {
        match crate::panic_recovery::catch_sync(
            crate::errors::PipelineSegment::ResponseRendering,
            render,
        ) {
            Ok(msg) => msg,
            Err(panic_event) => {
                tracing::error!(panic = %panic_event.message, "error renderer panicked; falling back to a bare text frame");
                Self::fallback_internal_message()
            }
        }
    }

    /// Hardcoded fallback frame when the regular renderer panics.
    ///
    /// A string literal, so none of the user code the renderer was calling
    /// runs again. It keeps the canonical envelope: a frame carries no content
    /// type, so a bare string would reach the client's decoder as text where
    /// every other error frame is an object.
    fn fallback_internal_message() -> WsMessage {
        WsMessage::text(r#"{"status":"error","kind":"Internal","message":"Internal Server Error"}"#)
    }

    /// Call the handler. A panic in it is caught here and becomes `PanicRecovered`,
    /// so user code cannot unwind into the adapter's read loop and take the
    /// connection down with it.
    async fn execute_handler(
        context: &WsContext,
        gateway: &Arc<Box<dyn Gateway>>,
    ) -> WsHandlerResult {
        let result = AssertUnwindSafe(gateway.handle_event(context))
            .catch_unwind()
            .await;
        match result {
            Ok(ExecutionResult::Ok(output)) => Ok(output),
            Ok(ExecutionResult::Err(ws_err)) => Err(ws_err),
            Err(payload) => Err(WsError::from(PanicRecovered::from_panic_payload(
                PipelineSegment::HandlerBody,
                payload,
            ))),
        }
    }

    pub(crate) async fn handle_disconnect(&self, client_id: String, reason: DisconnectReason) {
        let maybe = self.clients.write().remove(&client_id);
        if let Some(client) = maybe {
            tracing::debug!(client_id = %client_id, "WebSocket client disconnected");
            // An execution of its own, so teardown reads the session the way every other
            // participant does. No enhancers run: a disconnect cannot be rejected.
            let context = WsContext::new(
                client.clone(),
                WsMessage::text(""),
                "disconnect",
                Some(self.metadata.clone()),
            );
            self.gateway
                .on_disconnect(context.client(), reason, &context)
                .await;
        }
    }

    /// Parses the event name from a message using the gateway's `event_field()` key.
    fn extract_event(&self, message: &WsMessage) -> Result<String, WsError> {
        match message {
            WsMessage::Text(text) => {
                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(text) {
                    let field = self.gateway.event_field();
                    if let Some(event) = parsed.get(field).and_then(|v| v.as_str()) {
                        return Ok(event.to_string());
                    }
                }

                Err(WsError::InvalidMessage(format!(
                    "Missing '{}' field in JSON message",
                    self.gateway.event_field()
                )))
            }
            WsMessage::Binary(_) => Err(WsError::InvalidMessage(
                "Binary messages not yet supported for event extraction".into(),
            )),
            WsMessage::Ping(_) | WsMessage::Pong(_) | WsMessage::Close(_) => Err(
                WsError::InvalidMessage("Control frames don't have events".into()),
            ),
        }
    }

    pub(crate) async fn call_after_init(&self) {
        self.gateway.after_init().await;
    }

    pub(crate) fn namespace(&self) -> Option<String> {
        self.gateway.namespace()
    }

    pub(crate) fn port(&self) -> Option<u16> {
        self.gateway.port()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_event_from_json() {
        let wrapper = create_test_wrapper();

        let msg = WsMessage::text(r#"{"event": "message", "data": "hello"}"#);
        let event = wrapper.extract_event(&msg).unwrap();
        assert_eq!(event, "message");
    }

    #[test]
    fn test_extract_event_missing_field() {
        let wrapper = create_test_wrapper();

        let msg = WsMessage::text(r#"{"data": "hello"}"#);
        let result = wrapper.extract_event(&msg);
        assert!(result.is_err());
    }

    fn create_test_wrapper() -> GatewayWrapper {
        struct TestGateway;

        #[async_trait::async_trait]
        impl Gateway for TestGateway {
            fn token(&self) -> String {
                "TestGateway".to_string()
            }

            fn path(&self) -> String {
                "/test".to_string()
            }

            async fn handle_event(
                &self,
                _ctx: &WsContext,
            ) -> ExecutionResult<WsHandlerOutput, WsError> {
                ExecutionResult::Ok(WsHandlerOutput::Empty)
            }
        }

        GatewayWrapper::new(
            Arc::new(Box::new(TestGateway)),
            vec![],
            vec![],
            vec![],
            Arc::new(Metadata::new()),
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
        )
    }
}
