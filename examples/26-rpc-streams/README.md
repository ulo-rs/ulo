# 26 rpc streams

The signature picks the call shape: an `Inbound<T>` parameter is a streamed request, a stream return a streamed reply, both a duplex call. The client side is `stream`, `send_stream` and `duplex`.

A stream has no timeout of its own: `idle_timeout` bounds the silence between reply frames, `deadline` bounds the whole call and is sent to the server, and `send_stream` takes `timeout` like a request. Each ends the call with the kind `Timeout`.

```
cargo test -p ex-26-rpc-streams
PORT=7000 cargo run -p ex-26-rpc-streams
```

Read first:

- `src/lib.rs`: the five handlers, one per shape, a stalled stream and an endless one.
- `tests/streams.rs`: items in order for each shape; the idle timeout, the deadline and the `send_stream` timeout ending a call.

The timeouts in the tests are 150 to 300 milliseconds of real time: the client times its calls on the tokio runtime it is given, and a scripted clock cannot be given to it.

Next: [27 rpc deadlines](../27-rpc-deadlines).
