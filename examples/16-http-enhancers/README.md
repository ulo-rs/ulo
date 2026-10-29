# 16 http-enhancers

Guards, interceptors and an error handler at three tiers (global, controller, method), and
`#[meta(..)]` read by a guard through the handler's info.

```
cargo run -p ex-16-http-enhancers
curl -i http://127.0.0.1:3000/reports
curl -i -H 'x-role: admin' http://127.0.0.1:3000/reports/payroll
curl -i http://127.0.0.1:3000/reports/payroll
```

`/reports/payroll` declares `RequiresRole("admin")`; `RoleGuard` refuses a call without it and
`Refusals` answers the 403 in its own words. The server listens on `PORT` (default 3000).

Read first:

- `src/lib.rs`: `Stamp` is a guard and an interceptor; the global entries are `into AnyGuard<Http>`
  contributions in the module, the others are `#[guards]`/`#[interceptors]` on the impl and the method.
- `tests/enhancers.rs`: the full order of one call, the open route, and a refusal that stops everything below it.

Previous: [15 http-requests](../15-http-requests). Next: [17 http-pre-dispatch](../17-http-pre-dispatch).
