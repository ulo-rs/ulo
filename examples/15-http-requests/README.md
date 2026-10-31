# 15 http-requests

What a handler takes from a request and what it answers with: `Path`, `Query`, `Valid<Json<T>>`,
`Header<H>`, `Form`, `BodyStream` and a custom `FromCall` in; `Json`, `Created`, `WithHeaders` and a `Classify` error (also spelled through a `Result` alias) out.

```
cargo run -p ex-15-http-requests
curl -i -d '{"title":"milk"}' -H 'content-type: application/json' http://127.0.0.1:3000/notes
curl -i 'http://127.0.0.1:3000/notes?limit=1'
curl -i http://127.0.0.1:3000/notes/9
```

A new note answers 201 with a `Location`; a title outside 1 to 20 characters answers 422 with the
field named; an unknown id answers a 404 problem document. The server listens on `PORT` (default 3000).

`src/intake.rs` adds `Form`, a request-side `BodyStream` and `Caller`, an extractor written
in the example: implement `FromCall<Http>` and a handler takes the type. `Option<_>` of an extractor
is `None` for a missing value only. A handler is a method: one without `&self` (or `self: Arc<Self>`)
is refused at compile time (`crates/ulo-codegen-tests/tests/ui/handler_without_self.rs` holds the message).
`Host<T>`, a value an earlier stage put in the request, is in [17](../17-http-pre-dispatch).

Read first:

- `src/lib.rs`: the four handlers, `NewNote` with its `#[validate]` rule, and `NoteNotFound`, whose
  `#[classify(not_found)]` is the whole error mapping.
- `src/intake.rs`: `Caller`'s `FromCall` impl and the handlers that take the other extractors.
- `tests/requests.rs`: status, headers and body of each success and each refusal;
  `tests/intake.rs` does the same for the extractors above.
- `tests/binary.rs`: the compiled binary on a free port, ended with SIGTERM.

Previous: [14 without-macros](../14-without-macros). Next: [16 http-enhancers](../16-http-enhancers).
