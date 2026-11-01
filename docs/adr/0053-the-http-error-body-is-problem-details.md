# 0053 — The HTTP error body is RFC 9457 problem details

Status: withdrawn; never merged, the route table, the problem-details body and the response types are decided in `docs/experiments/di-redesign/transports/DESIGN.md`

Replaces `{statusCode, message, error, details?}` served as `application/json` with RFC 9457
problem details served as `application/problem+json`, in the change that collapses the six
hand-written copies of the envelope into one.

## Context

Every HTTP failure renders `{"statusCode": 404, "message": "…", "error": "Not Found"}`, optionally
with a `details` object merged in (`crates/ulo/src/http/error.rs:217-239`). The shape is NestJS's
`HttpException` body field for field. It has no standing outside this project, and RFC 9457
specifies the same thing: `type` / `title` / `status` / `detail` / `instance` under
`application/problem+json`, with `about:blank` as the `type` where only the status carries meaning.

This is F65, opened by the standards audit. That audit's own argument is the reason to act: of the
four transports, gRPC is the only one that speaks the specification owning it, and it was the only
one with neither defect the #310 error pass found.

The envelope is written by hand six times — once in core and once in each of the five adapters
(`axum_adapter.rs:207`, `actix_adapter.rs:162`, `poem_adapter.rs:165`, `rocket_adapter.rs:186`,
`salvo_adapter.rs:195`). ADR-0052 collapses those six into one. A wire shape is cheapest to change
in the change that already touches every copy of it, and nothing is published to crates.io yet.

## Decision

A failure renders:

```json
{
  "type":     "about:blank",
  "title":    "Unprocessable Content",
  "status":   422,
  "detail":   "name must not be empty",
  "instance": "/users"
}
```

served as `application/problem+json`.

- `status` comes from `Error::kind()` through `status_for`, as today.
- `title` is the status code's registered reason phrase from RFC 9110. This is where F69's
  correction arrives: 422 is "Unprocessable Content", not RFC 4918's "Unprocessable Entity".
- `detail` is `Error::message()`.
- `instance` is the request path, and is omitted where there is none.
- `type` is `about:blank` by default, which RFC 9457 defines as the value for a problem carrying no
  meaning beyond its status. An application naming its own problem types is left room and not built
  here.
- `Error::details()` becomes **top-level extension members**, which is the RFC's own mechanism for
  carrying additional fields, rather than one nested `details` object.

**`render_error` becomes reachable.** It is `pub(crate)` on every transport today, which is why a
handler catching its own error type cannot produce the canonical envelope (F87), and why an
application cannot restyle one without enumerating its concrete error types (F22). Rewriting the
renderer without making it reachable would leave both open against a shape no caller can produce.

## Consequences

- F65 closes. F69 closes as a consequence of `title` being the registered phrase.
- One renderer instead of six, since ADR-0052 removes the adapters' copies in the same change.
- A client matching on `error` or reading `statusCode` breaks. Nothing is published, so there is no
  one to migrate; after publication this would be a breaking change to every error response.
- A client checking `content-type === "application/json"` breaks, where one calling `.json()` does
  not. This is the most visible edge of the change.
- **Divergence across the four transports increases.** RPC and WebSocket keep
  `{"status":"error","kind":…}`, which #310 spent a pass making uniform. That is the correct
  outcome: a transport's wire shape is decided by the specification that owns it, not by what the
  other three do. Optimising for internal consistency is what produced three shapes with no
  standing outside the project. `ulo::Error` stays one trait and only the renderer differs, so the
  divergence is at the wire and not in the model.
- `#[catch]` handlers are unaffected: they receive the error, not the envelope.

## Roads not taken

**Keep the NestJS shape.** It is familiar to anyone arriving from Nest, and that is its whole case.
The project has already ruled on this class of argument: an ecosystem's popular framework is not a
standard, and copying NestJS is not a reason on its own.

**A feature flag or config toggle emitting the legacy shape.** Two shapes to implement, document and
test for as long as the flag exists, to serve a population of zero.

**Adopt RFC 9457 on all four transports.** It is an HTTP media type with HTTP semantics; carrying it
over an RPC broker or a WebSocket frame would be inventing a use the RFC does not define, which is
the same error as inventing an envelope — with a standard's name attached to make it look otherwise.
