# #146 — Extract handler parameters through FromContext

Merged 2026-08-15 into `master` from `feat/from-context-extraction`, commit [`6661593`](https://github.com/ulo-rs/ulo/commit/66615936cdc80ba927c88ecdd99df0b36a390daf).

Extractors stop being names the macro recognises and become types the compiler checks.

## What was wrong

An extractor was a table entry. `Path`, `Json`, `Query` and the rest were matched by name in the macro, and anything absent fell through to an `Unknown` arm that guessed — hand it the real body if nothing else wanted it, an empty body otherwise. Nothing about a type said which transports it was valid for, so an HTTP extractor in a WebSocket handler could only surface downstream, as noise.

## `FromContext<C>`

```rust
pub trait FromContext<C: HandlerContext>: Sized {
    type Error: Display;
    fn extract(ctx: &mut C) -> impl Future<Output = Result<Self, Self::Error>> + Send;
}
```

An extractor names the contexts it works with by which impls it carries, so a mismatch is now a trait bound failing where it is written:

```
error[E0277]: the trait bound `NotAnExtractor: FromRequestParts` is not satisfied
  --> src/orders.rs:10:1
help: the trait `FromRequestParts` is not implemented for `NotAnExtractor`
```

## What that required

Extraction reads from the context — that is what lets the trait be generic over one. So `Route::execute` takes the context alone; the dispatcher no longer lifts the body out and passes it alongside. Extractors take what they need from the context in signature order, and since nothing is moved out of a shared local any more, ordering carries no constraint of its own. The parts-before-body sort is gone.

## Writing an extractor

Two shorthands cover almost everything, and both produce a `FromContext` impl.

**Metadata** — `FromRequestParts`, unchanged, and it gets `FromContext<HttpContext>` from a blanket. **Existing custom extractors of this kind carry forward untouched.**

**Body** — `FromRequest` plus a small impl calling `extract_body`. Not a second blanket: two blankets over `T` overlap whatever their bounds, while a blanket and a concrete impl do not. Custom body extractors need that impl added — six lines, and the framework's six show the shape.

## Behaviour change worth naming

An enhancer that reads the body used to leave the handler an empty one. It now leaves an extraction failure naming the extractor that came up short:

```
`toni::extractors::bytes::Bytes` found the request body already read by something
earlier in this handler. The body can only be read once — take `Bytes` (or
`HttpRequest`) and parse it yourself if you need more than one view of it.
```

It renders as a 400 like any extraction failure, but the client's request was fine and the fault is the application's, so it also logs at error level.

## Also

The `Unknown` guessing arm is gone entirely — an unrecognised type is extracted through `FromContext` like any other. The diagnostic from #144 loses its "derive the rest from it" phrasing, which read as though the framework had a deriving mechanism; it now names the actual move.

gRPC is untouched — its handler signature is dictated by the tonic trait.
