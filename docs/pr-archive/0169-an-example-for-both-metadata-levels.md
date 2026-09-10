# #169 — An example for both metadata levels

Merged 2026-08-22 into `master` from `docs/metadata-example`, commit [`8e64d67`](https://github.com/ulo-rs/ulo/commit/8e64d67478aa4bda3580109af8e6778d515848fa).

`examples/route_metadata.rs` declared metadata on handlers alone and bound its guards to `HttpContext`. Both were what the attribute and the contexts allowed when it was written.

The impl block now declares `Roles` and a `RateLimit` for everything below it:

- `/health` declares `Public` on the handler, which applies there and nowhere else
- `/profile` declares nothing and inherits both
- `/admin/stats` and `/moderate` replace `Roles` and keep the block's `RateLimit`

The guards are written over `HandlerContext` rather than `HttpContext`, so `RolesGuard` reads the requirement from metadata and the caller from the extension bag, and registers unchanged on a `#[subscriptions]` or `#[patterns]` impl. What knows where a caller comes from stays transport-bound: a small `Guard<HttpContext>` reads the header and leaves a `Caller` in the bag.

Running it gives:

| path | admin | user | guest |
| --- | --- | --- | --- |
| `/api/health` | 200 | 200 | 200 |
| `/api/profile` | 200 | 200 | 403 |
| `/api/admin/stats` | 200 | 403 | 403 |
| `/api/moderate` | 200 | 403 | 403 |

The rate-limit check fires on every request that passes the roles guard, `/health` included — the block's `RateLimit` reaching a handler that declared a different type is the inheritance the example is there to show.
