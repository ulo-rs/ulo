# 23 ws gateway

A WebSocket gateway: one reply per message under the message's `id`, a message guard that writes an extension the handler reads, `OnConnect` and `OnDisconnect` hooks, and a drain that closes each connection with 1001.

```
cargo run -p ex-23-ws-gateway
```

The server listens on `PORT` (default 3000) at `/chat`. With any WebSocket client (`websocat` here), name yourself in the `x-name` header and send a message:

```
websocat -H 'x-name: ada' ws://127.0.0.1:3000/chat
{"event":"say","id":1,"data":"hello"}      # reply: {"id":1,"data":"ada: hello"}
```

Ctrl-C the server: the client receives close code 1001 with `the server is shutting down`.

Read first:

- `src/lib.rs`: the gateway, `Stamp` (a `Guard<Ws>` that inserts `Speaker`), the handler taking `Ext<Speaker>`, and the paired hooks writing to `Journal`.
- `src/badge.rs`: a connect guard (`Guard<WsConnect>`) writing a `Badge` into the connect phase's extensions. That bag reaches `OnConnect` only; each message is its own execution, so `on_connect` copies the badge into the session, where the `badge` message reads it. The session is built from the upgrade head before any guard runs, so a field written later, by `on_connect` or by the connect guard itself, needs interior mutability: here a `OnceLock`.
- `tests/gateway.rs`: every reply carries its message's `id` and the name of the connection that sent it; each connect has its disconnect, which still reads the session; the badge reaching `on_connect` and a later message; the drain's close frame, with the drain window on a scripted clock (`ScriptedTimer`).
- `tests/binary.rs`: runs the compiled binary and talks to it over a socket.

The gateway is declared `port = own`, so it needs a WebSocket server of its own, `ulo_ws_hyper::Server`; a gateway on the HTTP server's port goes through the upgrade hand-off `WsModule` registers.

Next: [24 ws rooms](../24-ws-rooms).
