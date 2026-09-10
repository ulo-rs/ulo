# #48 — feat(websocket): WsHandlerOutput — unified handler return type with stream support

Merged 2026-04-20 into `master` from `feat/ws-handler-output`, commit [`dea23fb`](https://github.com/ulo-rs/ulo/commit/dea23fbb8cad4b62748dafe1232732517a92e7c6).

## Problem

`handle_event` returned `Result<Option<WsMessage>, WsError>` — respond once
or don't respond. Any handler that needed to push multiple messages (streaming
results, server-initiated sequences) had to reach for `BroadcastService` as a
side-channel even when targeting only the calling client. Two output paths,
neither complete.

## What changed

`WsHandlerOutput` is a sum type with three variants:

- `Empty` — no response
- `Single(WsMessage)` — one response (same as before)
- `Stream(BoxStream<'static, WsMessage>)` — unbounded server-push sequence

`WsHandlerResult = Result<WsHandlerOutput, WsError>` replaces the old alias.
`From<WsMessage>` and `From<Option<WsMessage>>` keep migration mechanical.

The adapter layer receives `MessageCallbackResult::Stream` and spawns a task
per stream that drives it and writes each item to the client's sink. All stream
tasks are aborted on disconnect.

`BroadcastService` scope is unchanged — cross-client fan-out only.

## Design note — why the stream_slot exists

`BoxStream` is `Send` but not `Sync`. Storing it in `Context` (which must be
`Sync` for use across async boundaries) is not possible. Instead, `handle_message`
creates a `Arc<Mutex<Option<BoxStream>>>` side-channel (`stream_slot`) alongside
`Context`, threads it through the interceptor chain, and lifts the stream out
after dispatch. The context itself only ever holds `Option<WsMessage>` and stays
`Sync`.
