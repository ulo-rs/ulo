# #233 — Tell a refused WebSocket connection why

Merged 2026-09-02 into `master` from `feat/ws-connect-refusal-tells-the-caller`, commit [`2758e31`](https://github.com/ulo-rs/ulo/commit/2758e314e4c719942264d462e67e000052683cc5).

A connect guard's refusal reaches the caller. Guards run after the handshake, so there is no HTTP status left to refuse with, and every adapter answered by dropping the socket — `Err(_) => return`, with the `WsError` not even bound. The socket closed with no frame at all, which a client reports as an abnormal closure and cannot tell from a crash or a dead network. The framework logged at `error` because the log was the only place the refusal survived.

It now goes out on the socket the handshake opened: the canonical envelope as text, then a close carrying an RFC 6455 code. A browser reads the code and reason off `onclose`, which is more than a refused upgrade gives it — browsers withhold the status and body of a failed handshake.

- **Codes** — 1008 Policy Violation for a refusal the caller caused, 1013 Try Again Later for `TooManyRequests`, 1011 Internal Error for a server fault. A panicking connect guard keeps its typed `PanicRecovered` through the refusal, which is what puts it on the server-fault side.
- **Subprotocols** — `WsError::Refused { code, reason }` closes with a protocol's own code and sends no envelope. The graphql-ws gateway uses it for 4406, which its spec reserves for an unacceptable subprotocol; a client that has not agreed on the grammar cannot read an envelope written in it.
- **Frames** — `WsMessage::Close` carries `Option<CloseFrame>`, mapped onto the native close by all five WebSocket adapters. `close_with(code, reason)` truncates the reason to the 123 bytes RFC 6455 leaves for it, on a character boundary.
- **Logging** — a refused connect is narrated at `debug` like every other guard outcome, since the caller is now told. That closes the last row where the panic policy from ADR-0035 had an exception.

Breaking: a client that saw silence on refusal now sees one text frame and a close. `WsMessage::Close` gains a payload, so any code matching that variant needs the binding.

Implements ADR-0036.
