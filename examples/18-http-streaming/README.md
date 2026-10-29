# 18 http-streaming

A streamed body and Server-Sent Events: frames written while the handler runs, resumption from
`Last-Event-ID`, a keep-alive, and a stream that ends with the drain.

```
cargo run -p ex-18-http-streaming
curl -N http://127.0.0.1:3000/events &
curl -X POST http://127.0.0.1:3000/events/hello
curl -N -H 'last-event-id: 1' http://127.0.0.1:3000/events
```

The server listens on `PORT` (default 3000); Ctrl-C ends open streams and exits.

Read first:

- `src/lib.rs`: `Feed::since` is the stream, `events` adds `take_until` on the execution's `draining()`
  and `keep_alive`; `export` is a `Body::stream` paced by a semaphore.
- `tests/streaming.rs`: each frame read as it arrives, the keep-alive passed by a scripted clock, and
  the drain closing the stream with nothing abandoned.

Previous: [17 http-pre-dispatch](../17-http-pre-dispatch). Next: [19 http-uploads](../19-http-uploads).
