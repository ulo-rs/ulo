# 19 http-uploads

`Multipart` uploads under a server body limit and a tighter per-route `#[meta(BodyLimit(..))]`.

```
cargo run -p ex-19-http-uploads
echo hello > a.txt
curl -i -F doc=@a.txt http://127.0.0.1:3000/files
curl -i http://127.0.0.1:3000/files/a.txt
```

The server listens on `PORT` (default 3000) and refuses bodies over 1 KiB (256 bytes on `/avatars`).

Read first:

- `src/lib.rs`: `server` sets `.body_limit`, `/avatars` overrides it by metadata, and `store` reads the parts.
- `tests/uploads.rs`: the round trip, 413 by server limit and by route limit, and 400 for a broken boundary.

Previous: [18 http-streaming](../18-http-streaming). Next: [20 http-tls](../20-http-tls).
