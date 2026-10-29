# 12 standalone

An application with no server, as a job or a command runs it: `connect`, `execute` an async closure in its own execution, read per-execution data with `Ext<T>`, and `close(Signal::new("done"))`.
`ExecOptions::deadline` cancels the execution on the app's clock; the closure sees it through `exec.cancelled()`. Do not hold an `ExecutionRef` in an execution-scoped service: the execution then never ends and `close` waits out the drain window.

```
cargo run -p ex-12-standalone
```

Read first:

- `src/lib.rs`: `ReportJob` and `report`, which opens the execution.
- `tests/job.rs`: each `execute` has its own `Tenant`; `close` reports no abandoned or terminal executions and then refuses new ones; a deadline cancels the job on tokio's paused test clock.
- `tests/binary.rs`: runs the compiled binary to its end and checks what it prints.

Next: [13 testing](../13-testing).
