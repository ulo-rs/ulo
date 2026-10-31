# 17 http-pre-dispatch

Work done before routing: CORS, one tower layer, a middleware scoped to `/admin/*` that skips
`/admin/health`, and one that hands a value to the handler as `Host<T>`.

```
cargo run -p ex-17-http-pre-dispatch
curl -i -H 'origin: https://app.example' http://127.0.0.1:3000/
curl -i http://127.0.0.1:3000/admin/panel
curl -i -H 'x-api-key: sesame' http://127.0.0.1:3000/admin/panel
```

The server listens on `PORT` (default 3000).

`TagTenant` copies `x-tenant` into the request's extensions, as a host's auth layer would, and a
handler takes `Host<Tenant>`; a call without the header is a 500 for `Host<Tenant>` (a deployment
fault, the type stays server-side) and `None` for `Option<Host<Tenant>>`.

Read first:

- `src/lib.rs`: `EdgeModule` declares the stage as `m.meta::<PreDispatch>()`; the `#[module]` macro has
  no slot for it, so this module implements `Module` by hand; `TagTenant` and the two `/tenant*` routes.
- `tests/pre_dispatch.rs`: a preflight answered for a path with no route, the excluded route that never
  reaches the entry, the layer's header on a refusal and on a 404, and the `Host` value.

Previous: [16 http-enhancers](../16-http-enhancers). Next: [18 http-streaming](../18-http-streaming).
