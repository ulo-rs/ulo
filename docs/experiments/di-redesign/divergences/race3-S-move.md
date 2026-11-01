# Race 3, S-move: the shared suite crate (I11)

## Entries

1. **The `not_applicable` stamping rules stay in `conformance_suite!`.** The plan and I11 list them
   among the moved items. The rules are the `@stamper` arm of the RPC macro: they recurse through
   `$crate::conformance_suite!(@cases ..)` and name `$crate::cases::*`, so they cannot leave the
   macro without a redesign, which this move excludes. What moved is everything the rules lean on
   that is not RPC-specific (`Harness`, `Slots`, `PARALLEL_VAR`, `startup_failed!`). `ulo-db-conformance`
   writes its own stamper with the same `#[ignore = concat!("not applicable: ", ..)]` shape; a
   shared stamping macro is a follow-up if D3 wants one.
2. **`Broker`-bearing docs reworded.** Moved docs that linked `Broker` or `Broker::PARALLEL`
   (`Slots::hold`, `PARALLEL_VAR`'s paragraph, `startup_failed!`, the relay's module docs) name
   "the suite" or "the suite's environment" instead, since the support crate has no `Broker`. No
   behaviour changed.
3. **The two `bound` unit tests moved with `__private`.** They exist on `master`; no test is new.
4. **`ulo_rpc_conformance::__private` is now a re-export of the support crate's module.** The RPC
   macro still expands to `$crate::__private::Slots`, and `tests/harness.rs` still names
   `ulo_rpc_conformance::__private::Slots`; both resolve. `startup_failed` the function is
   reachable through it too, as before.
5. **`ulo-rpc-conformance` drops `ulo-runtime-tokio`, `ulo-runtime-smol`, `tokio` and `futures-channel`.**
   Nothing left in its `src/` or `tests/` names them; its `tokio` and `smol` features forward to
   the support crate's.
6. **`failures_dir` locates the workspace root by `ancestors().nth(2)` of this crate's manifest
   directory**, unchanged; `crates/ulo-conformance-support` sits at the same depth as
   `crates/ulo-rpc-conformance`.

## Requests

- Root `Cargo.toml` (S-skel): `crates/ulo-conformance-support` in `members`. No new workspace
  dependency: the crate uses `futures-channel` and `tokio`, both existing entries.
- `runtime-levels.txt` (S-skel): the two lines in the plan. The crate's default features pull
  `ulo` and `futures-channel` only; `tokio` and `ulo-runtime-tokio` are behind `tokio`.
