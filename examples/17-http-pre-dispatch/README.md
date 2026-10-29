# 17 http-pre-dispatch

Work done before routing: CORS, one tower layer, and a middleware scoped to `/admin/*` that skips
`/admin/health`.

```
cargo run -p ex-17-http-pre-dispatch
curl -i -H 'origin: https://app.example' http://127.0.0.1:3000/
curl -i http://127.0.0.1:3000/admin/panel
curl -i -H 'x-api-key: sesame' http://127.0.0.1:3000/admin/panel
```

The server listens on `PORT` (default 3000).

Read first:

- `src/lib.rs`: `EdgeModule` declares the stage as `m.meta::<PreDispatch>()`; the `#[module]` macro has
  no slot for it, so this module implements `Module` by hand.
- `tests/pre_dispatch.rs`: a preflight answered for a path with no route, the excluded route that never
  reaches the entry, and the layer's header on a refusal and on a 404.

Previous: [16 http-enhancers](../16-http-enhancers). Next: [18 http-streaming](../18-http-streaming).
