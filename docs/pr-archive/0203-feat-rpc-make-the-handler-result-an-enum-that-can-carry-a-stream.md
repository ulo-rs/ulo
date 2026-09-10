# #203 — feat(rpc): make the handler result an enum that can carry a stream

Merged 2026-08-29 into `master` from `feat/rpc-handler-output`, commit [`07c22c3`](https://github.com/ulo-rs/ulo/commit/07c22c3c5c70809536a0333bbb227588b77363cf).

Makes the RPC handler result an enum: `RpcHandlerResult = Result<RpcHandlerOutput, RpcError>` with `Empty`, `Single`, and `Stream` variants (ADR-0032). A returned stream is wrapped in `ScopedRpcStream` after the enhancer chain: the execution's cache, extensions, and cancellation token stay alive until the last item, and a drop before the end fires the token — the tail treatment HTTP and WebSocket carry (ADRs 0016/0021).

- Dispatcher: `RpcControllerWrapper::handle_message` takes `RpcCallInfo` whole, and the adapter's extension bag seeds the execution's — `RpcCallInfo.extensions` previously had no writer and no reader. `RpcContext::with_extensions` is the constructor that carries it; a unit test pins that the two handles address one bag.
- Macro: a handler may declare `-> RpcHandlerResult` and construct the output itself; the new classification passes it through untouched, checked before `returns_rpc_data`, whose fallback would otherwise claim the alias. Existing `RpcData`/serialize/event arms produce `Single`/`Empty`.
- Wire: `frame_response` takes the new shape. The stream grammar ships beside it — `frame_stream_item`/`frame_stream_end`/`frame_stream_error`/`frame_cancel`, `parse_reply_frame`, `drive_reply_stream`, and the runtime-free `Inflight` cancel registry — unit-tested; no adapter consumes it yet.
- Adapters: a stream answer on a transport that does not yet speak the grammar is refused with an `unsupported` wire error, and the scoped stream's drop fires the token — `a_stream_answer_is_refused_and_its_token_fires` pins both halves over TCP.

Migration: code matching `Ok(Some(d))` / `Ok(None)` becomes `Ok(RpcHandlerOutput::Single(d))` / `Ok(RpcHandlerOutput::Empty)`; `From<RpcData>` and `From<Option<RpcData>>` keep `.into()` working where the old shapes flowed. Error handlers still answer `RpcData`.

Per-transport streaming is deferred: each transport implements the grammar in its own change.
