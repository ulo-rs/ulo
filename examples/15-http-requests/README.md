# 15 http-requests

What a handler takes from a request and what it answers with: `Path`, `Query`, `Valid<Json<T>>`
and `Header<H>` in; `Json`, `Created`, `WithHeaders` and a `Classify` error out.

```
cargo run -p ex-15-http-requests
curl -i -d '{"title":"milk"}' -H 'content-type: application/json' http://127.0.0.1:3000/notes
curl -i 'http://127.0.0.1:3000/notes?limit=1'
curl -i http://127.0.0.1:3000/notes/9
```

A new note answers 201 with a `Location`; a title outside 1 to 20 characters answers 422 with the
field named; an unknown id answers a 404 problem document. The server listens on `PORT` (default 3000).

Read first:

- `src/lib.rs`: the four handlers, `NewNote` with its `#[validate]` rule, and `NoteNotFound`, whose
  `#[classify(not_found)]` is the whole error mapping.
- `tests/requests.rs`: status, headers and body of each success and each refusal.
- `tests/binary.rs`: the compiled binary on a free port, ended with SIGTERM.

Previous: [14 without-macros](../14-without-macros). Next: [16 http-enhancers](../16-http-enhancers).
