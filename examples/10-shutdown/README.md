# 10 shutdown

Stop accepting, drain, close. `drain_timeout` is the window in-flight requests get; `shutdown_timeout` caps the whole sequence. A handler that watches `ExecutionRef::draining()` ends its own work early, and `AppHandle::close(Signal)` starts the sequence from outside.
A request still held when the window ends is cancelled and answered 503 `the server is shutting down`; the `Shutdown` report counts it as abandoned.

```
cargo run -p ex-10-shutdown
curl http://127.0.0.1:3000/work    # held: nothing in the running binary releases it
```

The server listens on `PORT` (default 3000). Ctrl-C it while the `curl` waits: the `curl` gets 503 `the server is shutting down` when the 5 s window ends, and the server prints `closed on SIGINT: 1 abandoned`.

Read first:

- `src/lib.rs`: the two knobs, a gate that holds `/work`, and `/watch`.
- `tests/shutdown.rs`: a request released inside the window finishes and a new connection is refused; a request held past the window is answered 503 and counted in `Shutdown::abandoned`. The window runs on a scripted clock (`ScriptedTimer`), not tokio's paused one: a running server holds timers of its own, and the paused clock jumps to them while a request is in flight.
- `tests/binary.rs`: runs the compiled binary with `PORT` set to a free port, requests the address it prints and ends it with SIGTERM.

Next: [11 wiring report](../11-wiring-report).
