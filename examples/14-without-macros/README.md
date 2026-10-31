# 14 without macros

The macros expand to calls on the value API. This example writes example 06's providers, controller and modules by hand: `impl Construct` for each provider and for the controller, `impl Controller` whose `mount` registers the `GET /` route with an `ulo_http::HttpHandler`, and `impl Module` with `ModuleDef` for each module.

```
cargo run -p ex-14-without-macros
```

Read first:

- `src/lib.rs`: the hand-written `Construct` impls, the controller's `mount` and the modules.
- `tests/same_answers.rs`: one assertion body run against 06 and this example: the same answer through a `ModuleRef`, the same reply to `GET /` with the same handler reported for it, and the same wiring refusal for a read of the unexported binding.
- `tests/binary.rs`: runs the compiled binary to its end and checks what it prints.

Next: Part B starts with 15 (HTTP requests).
