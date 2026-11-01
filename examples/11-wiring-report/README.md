# 11 wiring report

`wire()` collects every wiring problem and returns them together. `BrokenModule` has one of each of five kinds: a duplicate binding, a missing dependency, an ambiguous one, a dependency cycle and a singleton reading per-execution data.

```
cargo run -p ex-11-wiring-report
```

Read first:

- `src/lib.rs`: the five mistakes.
- `tests/report.rs`: each problem as data (variant, key, modules, source location where the report has one), and the printed report as a human reads it.
- `tests/binary.rs`: runs the compiled binary to its end and checks what it prints.

Next: [12 standalone](../12-standalone).
