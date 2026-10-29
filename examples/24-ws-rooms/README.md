# 24 ws rooms

Rooms and broadcast: a connection joins a room, and a `Rooms` broadcast reaches every member of it and no one else. In one process the broadcast stays in memory; with `ulo-ws-redis` it reaches the members held by every process on the same Redis.

```
cargo run -p ex-24-ws-rooms                                                # one process, in memory
REDIS_URL=redis://127.0.0.1:6379 PORT=3000 cargo run -p ex-24-ws-rooms    # two processes on one Redis:
REDIS_URL=redis://127.0.0.1:6379 PORT=3001 cargo run -p ex-24-ws-rooms    # a shout on either reaches both
```

The server listens on `PORT` (default 3000) at `/rooms`. Messages: `join` (data: a room name), `shout` (data: `{"room", "text"}`), `ping`. A shout reaches members as `{"event":"shouted","data":"<text>"}`. Without `REDIS_URL` the adapter is in memory; with it, `WsModule::for_root().broadcast(Redis::url(..))`.

Read first:

- `src/lib.rs`: the gateway, and `Root`, which picks the adapter; nothing in the handlers changes between the two.
- `tests/rooms.rs`: one process; members of a room receive a shout, a member of another room and a connection in none do not.
- `tests/across_processes.rs`: two processes of the binary on one Redis; run with `cargo test -p ex-24-ws-rooms --features integration --test across_processes` (needs Docker; the container is `ulo-conf-ex24-redis`). The broker CI job runs it. The binary prints its address after `listen`, which returns once its Redis subscription is live, so the test's first shout is heard on the other process.
- `tests/binary.rs`: runs the compiled binary and pings it over a socket.

Next: [25 rpc basics](../25-rpc-basics).
