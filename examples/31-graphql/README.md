# 31 graphql

async-graphql served over HTTP. `GraphqlModule` mounts the endpoint, `AsyncGraphql<Api, Gql> as dyn Engine` binds the engine, and `Gql` is an execution-scoped context: every field of one request reads the same instance, and the next request gets a new one.

```
cargo run -p ex-31-graphql
curl -H 'content-type: application/json' -d '{"query":"{ greeting(name: \"ulo\") }"}' http://127.0.0.1:3000/graphql
```

The server listens on `PORT` (default 3000). A required variable left out answers 422, with the error naming the variable and its location and no `path`. The status is the same for a client accepting only `application/json`, and only a 2xx answer carries that media type; every other answer stays `application/graphql-response+json` (the default when `Accept` names nothing).

Read first:

- `src/lib.rs`: the schema, the `Gql` context, the engine binding, and the two modules (`engine_from` names the module that binds the engine).
- `tests/graphql.rs`: a query, a mutation read back by a query, the context built once per request, and the missing-variable request error with its status and body.
- `tests/binary.rs`: runs the compiled binary with `PORT` set to a free port, posts a query and ends it with SIGTERM.

Next: [32 graphql subscriptions](../32-graphql-subscriptions).
