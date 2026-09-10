# #204 — feat(rpc): let the client open a streaming call

Merged 2026-08-29 into `master` from `feat/rpc-client-open-stream`, commit [`15e6890`](https://github.com/ulo-rs/ulo/commit/15e6890928f98615a35eb78573abafd0b795dcce).

Gives the RPC client the consuming half of ADR-0032's streaming replies.

- `RpcClientTransport::open_stream(pattern, data, metadata)` joins `send`/`emit`, with a default body answering the new `RpcClientError::StreamingUnsupported` — implementations predating the stream grammar keep compiling and refuse loudly (pinned by `a_transport_without_the_grammar_refuses_a_stream_call`).
- `RpcReplyStream` is the reply: transports feed it through `RpcReplyStream::channel(capacity, on_cancel)`, and dropping it before the end runs `on_cancel` — the upstream disposal notice written once in core rather than per transport. An `Err` item is terminal (the transport produced or relayed it, no notice needed); a fully drained stream sends nothing. All three behaviors are unit-tested.
- `RpcClient::stream`/`stream_json` and `RpcRequest::stream`/`stream_json` sit beside the single-reply verbs; `stream_json` parses each item, yielding an `Err` in place of an item that does not parse.

The per-frame gap deadline (`with_timeout`, first frame included) and the cancel-notice wire form are each transport's contract, stated on `open_stream`; no transport implements the grammar yet.
