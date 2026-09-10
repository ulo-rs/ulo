# #243 — Read the one-body rule off the type, not the name

Merged 2026-09-05 into `master` from `feat/a-read-name-is-backed-by-a-type`, commit [`3788016`](https://github.com/ulo-rs/ulo/commit/37880166082160bc10da801fcb62e1eaf6f26faf).

Four macros decide what a handler parameter means by reading the last path segment of its written type. A name is not a type: `use toni::extractors::Bytes as B` gives a parameter whose segment reads `B`, and a handler is free to define its own `Json`. ADR-0040 states the rule those reads have to meet — **a name the framework reads is backed by a type it checks** — audits where it holds, and fixes the one place it did not.

Everywhere else, a name that lied fails to compile: a `Payload<T>` parameter receives a `toni::extractors::Payload`; a bare type on gRPC becomes the trait method's request type; a bare type on RPC is deserialised into, so it is bound by `DeserializeOwned`. HTTP's one-body rule was the exception, and nothing downstream caught a miss — reading the body is a runtime take from a shared context rather than a move, so an alias, a custom extractor and a `#[body]` marker were uncounted and the second reader found the body gone at request time.

`FromContext` now carries the fact, defaulted so an existing extractor keeps compiling:

```rust
pub trait FromContext<C: HandlerContext>: Sized {
    type Error: fmt::Display;

    /// Whether extracting this consumes what it reads, leaving nothing for a
    /// second extractor.
    const CONSUMES: bool = false;

    fn extract(ctx: &C) -> impl Future<Output = Result<Self, Self::Error>> + Send;
}
```

`#[routes]` emits one assertion per pair of parameters, reading the const off each written type — a pair rather than a sum, because a sum can say only that two of several parameters read the body while a pair names both:

```
error[E0080]: evaluation panicked: `dto` and `raw` both read the request body,
              and it can only be read once.
```

Four shapes are rejected at compile time that were not before, and one that fired before still does:

| Handler | Before | Now |
| --- | --- | --- |
| `Json<Dto>` + `Bytes` | compile error (name table) | compile error |
| `Json<Dto>` + a custom extractor declaring `CONSUMES` | 400 at request time | compile error |
| `#[body]` + `#[body]` | 400 at request time | compile error |
| `Json<Dto>` + `B`, an alias of `Bytes` | 400 at request time | compile error |
| `Query<Filter>` + `Json<Dto>` | accepted | accepted |

- **`Option<T>` and `Validated<E>`** forward `CONSUMES` from what they wrap, through their own impls rather than through anything the macro knows.
- **`HttpRequest`** gains the `FromContext<HttpContext>` impl it was being special-cased in place of, so every parameter is read the same way.
- **`BodyAlreadyRead` stays**, for what no signature can see: a guard that reads the body is not a handler parameter, and `an_enhancer_reads_the_body_once_and_sees_it_gone` pins that path.
- **Removed** with their last reader: `reject_second_body_extractor`, `ExtractorKind::takes_the_body`, and the inner-kind fields the recursion carried.

Two handlers with the same parameter names committing the same violation produce one diagnostic, because the messages are identical and rustc deduplicates them.
