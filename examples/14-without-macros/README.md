# 14 without macros

The macros expand to calls on the value API. This example writes example 06's providers and modules by hand: `impl Construct` for each provider and `impl Module` with `ModuleDef` for each module.
There is no controller. A route needs a handler value only the `#[routes]` attributes can build today.

```
cargo run -p ex-14-without-macros
```

Read first:

- `src/lib.rs`: the hand-written `Construct` impls and modules.
- `tests/same_answers.rs`: one assertion body run against 06 and this example: the same answer through a `ModuleRef`, and the same wiring refusal for a read of the unexported binding.
- `tests/binary.rs`: runs the compiled binary to its end and checks what it prints.

Next: Part B starts with 15 (HTTP requests).
