# 0054 — A response's status and headers are the ecosystem's types

Status: withdrawn; never merged, the route table, the problem-details body and the response types are decided in `docs/experiments/di-redesign/transports/DESIGN.md`

Replaces `HttpResponse.status: u16` with `http::StatusCode` and
`HttpResponse.headers: Vec<(String, String)>` with `http::HeaderMap`, so a response cannot carry a
status the protocol has no room for or a header the wire cannot take, and so two values under one
field name survive the trip to the adapter.

## Context

Both fields are primitives that each of the five adapters reinterprets on the way out. Probed
against master `654c7183`:

- **Status.** `HttpResponse.status` is a bare `u16` with no validation. Four adapters convert with
  `StatusCode::from_u16(..).unwrap_or(INTERNAL_SERVER_ERROR)`; rocket uses
  `Status::from_code(..).unwrap_or(Status::InternalServerError)`, which answers `None` for a code it
  does not know. A handler answering `599` produces `599` on axum, actix, salvo and poem, and `500`
  on rocket (F114). A handler answering `9999` produces `500` everywhere, with nothing said.
- **Headers.** The field is an ordered list that admits repeats, and four adapters flatten it with
  `insert`, which replaces. A handler pushing two `Set-Cookie` values sends both on poem and only
  the last on the other four (F108). RFC 9110 §5.3 notes that `Set-Cookie` "cannot be combined into
  a single field value", so two cookies require two field lines and four adapters cannot produce
  them. The live case is not cookies: `CorsMiddleware` pushes `Vary: Origin` after the handler has
  run, and a handler's own `Vary: Accept-Encoding` is replaced on four adapters.
- **Invalid headers.** A value carrying CR or LF, or a name that is not a token, reaches five code
  paths: dropped without a word on axum, dropped with a `warn` on salvo and poem, a `500` on actix,
  and zero bytes with the connection closed on rocket (F113). No adapter wrote the injected line, so
  what differs is the refusal rather than the safety — and in no case does the application learn.

The request side is already typed: `HttpRequest` is `http::Request`, so `http` is a public
dependency of this crate today. The response side is the inconsistency.

## Decision

```rust
pub struct HttpResponse {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Option<Body>,
}
```

with `StatusCode` and `HeaderMap` re-exported from `ulo::http`.

- `StatusCode::from_u16` admits 100–999, which is exactly RFC 9110's three-digit range, so a private
  status stays reachable and `9999` stops being constructible. Rocket moves to `Status::new`.
- `HeaderMap` is a multimap: `append` adds a field line and `insert` replaces one, and the choice is
  made at the call site instead of by the adapter. Order within a name is preserved, which is the
  order RFC 9110 §5.3 calls significant; order across names is not preserved and the same section
  says it does not matter.
- The eight named constructors (`ok`, `created`, `no_content`, …) are unchanged and remain the
  ergonomic path. The field stays `pub`: with typed values there is no invariant left to protect.
- The builder's fallible path takes the shape this project already settled on gRPC —
  `set_header(&mut self, key: &str, value: &str) -> Result<(), InvalidHeader>`
  (`crates/ulo/src/grpc/runtime.rs:242`). A dynamic header value is caller data, so it is refused
  with an error rather than a panic, and the application is told.

## Consequences

- F108, F113 and F114 close. F109 (`CorsMiddleware` omitting `Vary: Origin`) and F115 (no cookie
  surface) become implementable — neither is correct while a push can replace without saying so.
- The adapters' response conversion becomes a move rather than a per-pair re-parse, which is where
  the five refusal behaviours lived. Actix converts through `from_u16(code.as_u16())` because
  actix-http pins `http` 0.2 against ulo's 1.4; that is what it already does with a `u16`.
- **Call-site cost, counted.** The tree holds 10 numeric `.status(n)` calls and eight of them are
  inside `http_response.rs` defining the named constructors; the other two are in integration tests.
  The migration is the 26 `status: <n>` struct literals — 10 in the two GraphQL controllers, 9
  emitted from `quote!` inside `ulo-macros`, which name a path anyway, and 7 across core and the
  actix adapter.
- A caller building `HttpResponse` by struct literal writes `StatusCode::OK` rather than `200`.
- The dynamic case improves: a proxy forwarding an upstream status gains a way to say the number was
  not a status, where today it becomes 500 on five adapters with nothing said.

## Roads not taken

**Keep `u16` and fix rocket alone.** One line, and it makes the five agree. It leaves `.status(9999)`
compiling and rendering 500 everywhere — F114's shape, agreed upon rather than removed.

**A ulo status enum or newtype.** Status codes are an IANA registry and RFC 9110 has a client treat
an unrecognised code as `x00`, so a closed set carries the defect F70 names for `HttpMethod`. A
newtype would need `Display`, `FromStr`, serde and a conversion per adapter, arriving at a worse
`http::StatusCode`.

**Keep the `Vec` and make every adapter `append`.** It fixes `Set-Cookie` and fixes nothing else:
the values stay unvalidated, so F113's five refusals survive, and the caller still cannot say
"replace" where replacing is what it means.
