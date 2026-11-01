# 21 http-in-axum

A ulo app mounted inside an axum server: the host owns the socket and its own routes, the app answers
under `/api`.

```
cargo run -p ex-21-http-in-axum
curl http://127.0.0.1:3000/health
curl http://127.0.0.1:3000/api/hello
```

The server listens on `PORT` (default 3000).

Read first:

- `src/lib.rs`: `embedded()` is the server the app binds to, `router` nests its service beside an axum route.
- `src/main.rs`: `ulo_http_axum::run` serves the host under the app's one shutdown trigger.
- `tests/embedded.rs`: both routes on one socket, and a request still running at the end of the drain
  answered 503 with the shutdown phrase.

Previous: [20 http-tls](../20-http-tls). Next: [22 http-on-smol](../22-http-on-smol).
