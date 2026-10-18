# Divergences: race 2b, tests batch 41, the NATS roots once per process while any link holds them, and a `nats://` link that does not wait for them (F427)

The sixty-fifth response, signed off 2026-10-18, answers `race2b-tests39.md` and `race2b-tests40.md`
and changes decision 139. F427's read mechanism was probed first: at `460de797` a `tls://` client
whose first call's 100 ms timeout ended during a 600 ms load, and whose second call waited for one,
ran two loads. The load is now a task on the blocking pool of the link that starts it, every holder
awaiting one shared answer, and the process keeps a `Weak` to it: a link needing roots while another
link or a connection holds the load joins it, and once the last holder drops it the file is removed
and the next link loads again. A `tls://` link waits for the load; a `nats://` link waits for
nothing, handing a connection the file once the load has ended and connecting without it before.
Thirty links preparing at once ran one load and held one file, gone once the thirty dropped; a call
cut short by its timeout during a load left that load running for the next call; a `nats://` server
and client bound, connected and carried a request in 8.8 to 15.0 ms while their load blocked for
twice the suite's 5 s boot budget. Each test fails against a break: thirty loads with a load per
link, a second load with the load kept only once it answers, 10.02 s with the `nats://` link
waiting, and the file outliving the links with the slot made strong. The NATS conformance suite
passed 20 of 20 runs on this Mac, 6.19 to 11.38 s, one load per run of 0.20 to 0.31 s; six runs of
the link as `460de797` has it, in the same session, failed 9 to 50 scenarios each. The suite's
times were measured beside another batch's builds, load averages 6 to 12, and are not comparable
with batch 38's 5.7 s. F423 holds again for one connection, a `nats://` connection made before the
load ends that its server upgrades, as the response accepts and the documentation now states.

Files changed: `crates/ulo-rpc-nats/src/link.rs`, `src/lib.rs` and `Cargo.toml`'s comment on
`rustls-native-certs`. `Cargo.lock` is unchanged. The ledger's text is in this log's last section
for the coordinator to paste: fix notes under F427, F423, F422 and F417, their commit written
`<commit>`. The batch started from `460de797` and made no commit. Versions are unchanged at
`0.1.0-alpha.1`.

## The signatures

No public signature changes. Inside `ulo-rpc-nats`:

```rust
// crates/ulo-rpc-nats/src/link.rs, crate-private
pub(crate) struct Roots { pub(crate) own: Option<Loader>, pub(crate) resolve: fn() -> Result<(), String>, .. }
pub(crate) struct Loader { pub(crate) load: fn() -> CertificateResult, pub(crate) dir: fn() -> PathBuf }
pub(crate) struct Load { .. }                     // one load, its answer shared through a `watch`
static PROCESS: Mutex<Weak<Load>>;                // the process's load
#[cfg(test)] pub(crate) static PROCESS_LOADER: Mutex<Loader>;
async fn open(servers: Vec<ServerAddr>, options: ConnectOptions, roots: Option<PathBuf>) -> Result<Client, BoxError>;
```

`Roots::load`, `Roots::dir`, `Roots::warned` and `Roots::written` are gone; the server and client
sides hold `Option<Arc<Load>>` where they held `Option<Arc<RootsFile>>`.

What changes in meaning without a signature:

- One load and one roots file per process at a time, while any link or connection holds them; a
  link needing roots after the last holder dropped loads again.
- A server's `prepare` and `listen` and a client's `connect` on a `nats://` url no longer wait for
  the load. On `tls://` they wait as before, and refuse as before.
- A failed load is kept while the load is held, by any link in the process, where it was kept for
  one link's life.
- A `tls://` link resolves rustls's provider on the thread awaiting its first need, before it joins
  the load, where the resolution ran in the blocking task.

## Decisions

Line numbers are those of the batch's tree.

### 1. The premise, by probe

F427 read its mechanism and did not probe it: `Roots::load` ran the load inside
`OnceCell::get_or_try_init` in the caller's future, so a call that gave up dropped the future, and
the cell, never filled, had the next call start a new load. p01 (`probes/p01-0.txt`, the probe a test
inserted into `link.rs` at `460de797` for its run): a `tls://` client whose loader blocks 600 ms, a
first call with a 100 ms timeout, then a second with 5 s. The first failed `Timeout` after 102.8 ms
with one load begun; the second failed `Unavailable`, nothing listening, 711.1 ms after the first
began, with two loads run. A blocking task outlives the future awaiting it, so the first load ran to
its end and its result was discarded.

p04 (`probes/p04-0.txt`) is decision 3's premise at `460de797`: with roots taking 600 ms, a server's
`prepare` took 607.6 ms on `nats://localhost:1` and 605.6 ms on `tls://localhost:1`, so every
scheme waited for the load in `prepare`.

### 2. The load is a task with a shared answer

`Load::start` (`crates/ulo-rpc-nats/src/link.rs:1062`) spawns the load on the link's runtime's
`spawn_blocking` and returns at once; the task writes its answer, the file or the refusal's cause,
into a `tokio::sync::watch` channel whose receiver the `Load` holds. Every waiter clones the receiver
and awaits it (`Load::wait`, `:1084`), so a waiter that gives up drops its clone and nothing else.
The task's `send` fails once every holder has dropped the load, and the file drops with the refused
value on the blocking thread. A panic in the loader is caught inside the task with `catch_unwind`
and kept as "loading them panicked: ..", as the join error's panic was before; a task the runtime
drops unrun, its runtime shut down, leaves the channel closed with no answer, and `abandoned`
(`:1079`) has the next need start a new load, which was the old cell's "not kept" case.

A `watch` was chosen over a shared future because the answer is read synchronously too, by
`Roots::ready` and `Load::path`, without a poll.

### 3. One load per process while any link holds it

`PROCESS` (`:953`) is a `Mutex<Weak<Load>>`. `Load::shared` (`:1050`) upgrades it and returns the
live load, or starts one on the calling link's runtime and stores its `Weak`. Each link keeps an
`Arc<Load>` from its first need (`Roots::held`, `:974`), and a server or client side keeps one while
its connection was handed the file, as it kept the `RootsFile` before. The file lives inside the
channel's value, so it goes when the last `Arc<Load>` and the task have dropped. A strong slot would
keep the file for the life of the process; the response rules it out. The loader a test sets on a
link, `Roots::own`, gives that link a load of its own and bypasses the slot, so every existing test
counts the loads it counted.

Thirty links needing roots at once join the first one's load, whichever runtime started it; a link
on another runtime waits on the same channel. The load runs on the starting link's runtime, and if
that runtime shuts down before the task runs, the load is abandoned and the next link replaces it.

### 4. A `nats://` link waits for nothing

`Nats::roots` (`:164`) branches on the scheme. For a url naming a `tls://` server, `Roots::wait`
(`:990`) resolves the provider, waits for the load and refuses a failed one as `Misconfigured`,
naming `tls://`, as before. For a url naming none, `Roots::ready` (`:1005`) starts or joins the load
and answers at once: the load if its file is written, `None` while it runs or after it failed. A
server's `prepare` therefore returns at once on `nats://`, 0.72 ms against 604.1 ms on `tls://` with
600 ms roots (`probes/p05-0.txt`), and `listen` and every client `connect` pass the file when it
exists and connect without it otherwise. `open` (`:908`) takes the path.

async-nats fixes a connection's root files when it connects and reads them at every TLS connect and
reconnect of that connection: its `Connector` keeps a `ConnectorOptions` whose `certificates` come
from the `ConnectOptions` it was built with (async-nats 0.46.0 `src/connector.rs:57`, `:78`), and
no public method changes them, which is read, not probed. A connection made before the load ends
therefore keeps no file at its reconnects either; if its server upgrades it, async-nats reads the
platform's store on the thread polling it, each time. A later connection, a client's after its reply lane ended or any new link's,
is handed the file. Handing every connection a path reserved before the load would give its
reconnects the file, at the cost of the first upgraded connection failing on a missing or empty
file, and of a predictable name a process sharing the temp directory could plant first (S1).

The `warn` for a `nats://` link whose load failed now comes once per load: `Roots::ready` logs it
when it finds the load failed, and when the load still runs it has one task per load
(`warn_when_failed`, `:1101`) wait for the answer and log a failure (S3). The message and its cause
are as batch 40's fold has them, the cause naming no scheme.

### 5. The provider is resolved per link, before the load

The load serves `tls://` and `nats://` links alike, so the provider check, which runs for `tls://`
alone, left the load's task. `Roots::wait` runs `resolve`, `resolve_provider` unless a test replaces
it, once per link through a `OnceLock`, on the thread awaiting the link's first need and before the
link joins the load, and refuses it as before. `ClientConfig::builder()` does no I/O; with a
provider installed or named by rustls's features it returns at once (S2).
`a_tls_link_whose_provider_does_not_resolve_is_refused_naming_the_fix` still finds no load, and
`tests/two_providers.rs` passes unchanged.

### 6. The tests

New:

- `thirty_links_at_once_share_one_load_and_one_file` (`:1854`): the process's loader set to the test
  certificate after 300 ms, written in a directory of the test's own; thirty `tls://` server links
  `prepare` at once through `join_all`. One load, every link holding the same path, the directory
  holding that one file; the thirty dropped, the directory empty within `emptied`'s five seconds; a
  thirty-first link then loads again, a second load, one new file, removed with it.
- `a_call_that_gives_up_while_the_roots_load_leaves_the_load_running` (`:1885`), item 2's test: a
  `tls://` client on a gated loader, the gate opened by the test or after 10 s. A first call with a
  100 ms timeout fails `Timeout`; a second call, spawned, is still waiting 100 ms later with one load
  begun; the gate opens, the second call fails `Unavailable`, one load in all and one file, removed
  with the client.
- `plain::a_plain_link_connects_within_the_boot_budget_while_its_roots_load` (`:1746`), behind
  `integration`, against the plaintext NATS container: the process's loader gated for `HELD`, 10 s,
  twice `Budget::default().boot`, which the test asserts. From the app's build, a `nats://`
  server's `prepare` and `listen`, a client's `connect` and one request reaching the server finish
  within the boot budget, with one load begun and neither link holding a file. Then the gate opens:
  the client's link settles on a file, the server's holds the same one, `Nats::roots` hands it to a
  later connect, a later `connect` succeeds, one load and one file; both links closed and dropped,
  the directory empties.

The gate is opened once the round trip is measured, not left to block the full 10 s; the break in
decision 7 shows the full block (S6).

The tests that use the process's load take turns through `ProcessTurn` (`:1404`), a
`tokio::sync::Mutex` that sets `PROCESS_LOADER` for the turn and puts the platform loader back when
the turn drops, and that starts a turn only once no process load is held, an earlier turn's holders
given five seconds (S7).

Changed:

- `two_links_write_files_of_their_own` is `two_loads_write_files_of_their_own` (`:1591`): two links
  in one process now share a file, and what the test holds, unique names for two loads, still
  applies, since a load whose holders have dropped can still be writing its file when the next
  starts.
- `a_plain_link_writes_its_roots_too`, `a_plain_link_whose_roots_did_not_load_is_not_refused`,
  `a_plain_link_resolves_no_provider`, `plain::a_nats_url_to_a_plain_server_stays_plain` and
  `plain::a_closed_app_s_server_removes_its_roots` read the file or the load count once the load has
  answered, through `written`, `counted` and `Roots::settled`, each waiting at most five seconds: a
  `nats://` link's connect no longer waits for it.
- `over_tls::connecting_on_a_nats_url_to_a_server_requiring_tls_does_not_stall_the_thread`: its
  links join a process load that `TlsNats::start` makes and awaits before the scenario starts, so
  each connection is made after the load has ended and is handed its file, and the four links load
  once, where each loaded once of its own. Run as before, a load of each link's own, the server's
  link connected before its 50 ms load ended and the server's `bind` failed "invalid peer
  certificate: UnknownIssuer", async-nats having trusted the platform's store for the upgrade
  (p07, `probes/p07-0.txt`; S5).

`a_plain_link_s_logged_cause_does_not_name_tls` calls `Roots::wait` in place of `Roots::load`.
`over_tls::connecting_over_tls_does_not_stall_the_thread` and
`over_tls::reconnecting_over_tls_does_not_stall_the_thread` keep a load per link and their counts,
four and two.

### 7. The breaks

Each break went through `batch41/scripts/brk.py` in the mirror (decision 9), the file copied aside
before the edit and compared with `cmp` after the restore, against the mirror's copy and the real
tree's:

- b01, the process's load replaced by a load per link (`Roots::held`'s `None` arm starting a load):
  the thirty-links test failed, 30 loads against 1 (`broken/b01-0.txt`).
- b02, a `nats://` link waiting for its load (`Nats::roots`'s plain arm calling `Roots::wait`): the
  boot-budget test failed, "binding, connecting and one request took 10.019089458s, past the boot
  budget of 5s" (`broken/b02-0.txt`).
- b03, the load kept only once it answers, the old cell's shape (`Roots::wait` storing the load in
  `held` after its answer): the shared-answer test failed, a second load begun, 2 against 1
  (`broken/b03-0.txt`).
- b04, the slot kept strong (an `Arc` leaked beside the `Weak`): the thirty-links test failed, "the
  file outlived the thirty links" (`broken/b04-0.txt`).

Every restore reported `ok` with both `cmp`s identical.

### 8. The NATS suite

`cargo test -p ulo-rpc-nats --features integration --test conformance --locked`, in the mirror,
after `cargo clean -p ulo-rpc-nats` and a build whose output names `Compiling ulo-rpc-nats`:

- 12 runs (`suites/nats-fixed-*.txt`): 12 passed, 54 passed and 1 ignored each, libtest's 6.70 to
  11.38 s, no roots file left by any.
- 8 runs on the final tree (`suites/nats-final-*.txt`): 8 passed, 54 and 1 each, 6.19 to 9.59 s,
  load averages 5.8 to 7.3 over the minute, none left a file.
- p03 (`probes/p03-*.txt`), three runs with the load's start and answer printed: one load per run,
  answering after 212.5, 201.1 and 310.0 ms, the suite passing in 7.57, 8.23 and 8.88 s.
- p02, the control (`probes/p02-*.txt`), six runs with `link.rs` as `460de797` has it, between the
  first measurement and the twelve: six failed, 11, 9, 10, 40, 50 and 50 scenarios, 18.11 to
  45.87 s, most on the boot check's "the server did not answer within the boot budget".

The twenty passing runs' median is 7.7 s against batch 38's 5.7 s and batch 40's 6.0 to 6.5 s with
the store's load replaced. The fastest, 6.19 s, sits in batch 40's range. Batch 42's agent was
building and testing the workspace on the same host through the runs, the load averages 6 to 12, so
the difference is not attributed to the link (S10).

A first measurement of twelve runs and a second of six are kept in `superseded/suites-stale/` and
counted in nothing above. The sync into the mirror used `rsync -a`, which put the real tree's older
mtime back on a `link.rs` a break had restored, so cargo kept the binary the control had built: the
second six ran `460de797`'s link, built no `ulo-rpc-nats`, failed 30 and then 50 scenarios five
times, and left 285 roots files. The first twelve may have run a build from before the restore. The
sync now copies by checksum and keeps no mtime, and each measurement starts from
`cargo clean -p ulo-rpc-nats`.

### 9. Building in a mirror

At the batch's start the workspace did not build: batch 42's edits in `crates/ulo`, in progress,
named a private function (`superseded/p01-0-real-tree-build-failed.txt`). The batch built and ran
everything in `batch41/mirror`, an export of `460de797` through `git archive`, with
`crates/ulo-rpc-nats` copied from the real tree before each run and compared with `diff -r`. The
break runner's tree hash covers the mirror's `crates/ulo-rpc-nats`, `Cargo.toml` and `Cargo.lock`.
The first p01 run, in the real tree, is where the original runner's whole-`crates` hash reported
`MISMATCH`, the other batch's edits having changed the tree during the run while `link.rs` and
`Cargo.lock` compared identical. The workspace test ran in the real tree at the end (S8).

## The tests

- **F427:** `ulo-rpc-nats`'s `link::tests::thirty_links_at_once_share_one_load_and_one_file` and
  `link::tests::a_call_that_gives_up_while_the_roots_load_leaves_the_load_running`; behind
  `integration`, `link::tests::plain::a_plain_link_connects_within_the_boot_budget_while_its_roots_load`.
  Decisions 2 to 4 and 6.
- **F423, narrowed:** `over_tls::connecting_on_a_nats_url_to_a_server_requiring_tls_does_not_stall_the_thread`
  on a load finished before its links connect. Decision 6.

### Before and after

The probes and breaks went through `batch41/scripts/brk.py`, batch 40's runner pointed at the
mirror, which now also copies each touched file aside and compares it with `cmp` after the restore,
run one spec at a time by `batch41/scripts/breaks.sh`, which records the container list before and
after each. Specs in `batch41/specs/`, output in `batch41/probes/` and `batch41/broken/` with
`summary.txt` and `runner.txt`; the suite runs by `batch41/scripts/nats_reruns.sh` in
`batch41/suites/` with `summary.txt`. Every restore reported `ok`, and every container list after a
spec or a suite run matches the one before it.

| Run | Target | Result |
| --- | --- | --- |
| p01. a `tls://` call cut short during a 600 ms load, then another, at `460de797` | the link's unit tests | two loads, the second call ending 711.1 ms after the first began |
| p02. the NATS suite with the link as `460de797` has it | six runs | six failed, 9 to 50 scenarios, 18.11 to 45.87 s |
| p03. the NATS suite, each load printed | three runs | one load a run, 201.1 to 310.0 ms; 54 passed each |
| p04. a server's `prepare` per scheme with 600 ms roots, at `460de797` | the link's unit tests | 607.6 ms on `nats://`, 605.6 ms on `tls://` |
| p05. the same with the fix | the link's unit tests | 0.72 ms on `nats://`, 604.1 ms on `tls://` |
| p06. the boot-budget test's time printed | three runs | 8.8, 15.0 and 11.2 ms |
| p07. the `nats://` TLS stall test on a load per link | the link's unit tests | the server's `bind` failed `UnknownIssuer` |
| b01. a load per link | the thirty-links test | failed, 30 loads |
| b02. a `nats://` link waiting for its load | the boot-budget test | failed, 10.02 s |
| b03. the load kept only once it answers | the shared-answer test | failed, two loads |
| b04. the slot kept strong | the thirty-links test | failed, the file left |
| the NATS suite, the fix | twelve runs, then eight on the final tree | 20 passed, 6.19 to 11.38 s |
| the restored tree | every target above | every test passed |

## Left for the DESIGN folds

Line numbers are those of the transports DESIGN at `460de797`.

- Decision 139 (line 1665): "once per link" becomes once per process while any link or connection
  holds the load: a `Weak` process slot, each link holding an `Arc<Load>` from its first need, the
  load a `spawn_blocking` task whose answer every holder awaits through a `watch` channel, a caller
  giving up dropping its own wait, the file dropped with the channel's value once the last holder
  and the task have dropped it, and a link needing roots after that loading again. The per-link
  seam, `Roots::own`, gives a test's link a load of its own; `PROCESS_LOADER` is the process's.
  The removal is now on whichever thread drops the last holder or on the blocking thread whose
  `send` is refused. A kept failure is kept while the load is held. The unit-test list gains the
  thirty-links and shared-answer tests, and the two-links test is `two_loads_write_files_of_their_own`.
- Decision 144 (line 1670): a `nats://` link waits for no load, in `prepare`, `listen`
  or `connect`; a connection made before the load ends gets no file, and its reconnects none, so a
  server's upgrade of it has async-nats read the store on the polling thread (F423 narrowed); the
  `warn` comes once per load, from a task when the load still runs; the plain-link tests read the
  file once the load has answered; the boot-budget test.
- Decision 145: the provider resolution runs once per link, on the thread awaiting the link's first
  `tls://` need, before the link joins the load, not in the load's task.
- Decision 140: `over_tls::connecting_on_a_nats_url_to_a_server_requiring_tls_does_not_stall_the_thread`
  joins a process load `TlsNats::start` finished before the scenario, one load.
- §5.3 and §12, where they say a link writes a file of its own.
- `Misconfigured`'s documentation in `crates/ulo-rpc/src/link.rs:431` says "a NATS link that could
  not write them to its file"; the file is the process's. Left unchanged, `ulo-rpc` being outside
  the batch's crate (S11).

## Needs sign-off

Numbered from S1 for this batch.

### S1. A connection made before the load ends keeps no file at its reconnects

Decision 4. The response says later connects and reconnects use the file once it exists; async-nats
fixes a connection's root files at its connect, so the batch reads that as later connections and
their reconnects. A connection made without the file, if its server upgrades it, has async-nats read
the platform's store at that connect and at each of its reconnects. A server's link connects once,
in `listen`, often before a cold load ends, so on a server that upgrades `nats://` connections the
server's own connection is the one F423 now covers. The alternative, a path reserved before the
load and handed to every connection, makes that connection fail its upgrade on a missing or empty
file before the load ends, and a name known ahead in a temp directory others can write can be
planted with roots of their choosing.

### S2. The provider is resolved on the caller's thread, once per link

Decision 5. The load is shared across schemes and the resolution is for `tls://` alone, so it left
the load's task. `ClientConfig::builder()` does no I/O. In a build linking two providers with none
installed, rustls panics inside `catch_unwind` and the panic hook prints its message on the caller's
thread, once per link, as it did on the blocking thread.

### S3. A failed load is logged once per load, not once per link

Decision 4. With one load per process, a failure is logged once by the first `nats://` link to find
it, or by a task one `nats://` link starts while the load still runs. That task holds the load until
the answer comes, and ends with it.

### S4. A failed process load is kept while it is held

Decision 3. A failure is kept while any link holds the load and retried by the first link to need
roots after the last holder drops, where a link kept its own failure for its whole life. An
application's links usually live as long as the app, so in practice the failure is kept for the
app's life.

### S5. The `nats://` TLS stall test runs on a finished load

Decision 6. Its links join a load the broker finished before the scenario. The case F423 is
narrowed to, an upgraded connection made before the load ends, has no test: run on this host it
reads the platform's store, whose timing and contents are the runner's.

### S6. The boot-budget test opens its gate once it has measured

Decision 6. The loader blocks for at most `HELD`, 10 s, twice the boot budget, and the test opens
the gate once the round trip is measured, checking that the load had not ended when it was. b02
shows what a link waiting for it costs, the full 10 s.

### S7. The process's loader seam and the turn lock

Decision 6. `PROCESS_LOADER` is a `#[cfg(test)]` static the unit tests set, and the tests that use
the process's load take turns through `ProcessTurn`. The test binaries `tests/conformance.rs`,
`tests/two_providers.rs` and `tests/handle.rs` read the platform's store through the process's
load.

### S8. The batch built in a mirror of `460de797`

Decision 9. Every probe, break, suite run and check ran on `460de797` with this batch's crate, not
on the real tree with batch 42's edits; the workspace test ran on the real tree, batch 42's edits
included.

### S9. Roots files the batch left in the temp directory

Decision 8. 462 entries in the temp directory are newer than the batch's start: 458
`ulo-rpc-nats-roots-*.pem` files and 4 `ulo-rpc-nats-roots-test-*` directories. 173 files are the
control's six runs and 285 the six runs on a stale binary (`verify/roots-left-by-batch41.txt`, by
process). The directories are b01's to b04's, each test failing before its own cleanup; three are
empty, and b04's holds the file its leaked load wrote. None of the twenty passing suite runs left
a file. The batch removed nothing; the temp directory holds 1,476 roots entries in all.

### S10. The suite's time beside another batch's builds

Decision 8. Median 7.7 s against batch 38's 5.7 s, measured while batch 42 built and tested the
workspace on the host, load averages 6 to 12. The load itself is one per run, 0.2 to 0.3 s, on the
blocking pool. A run on an idle host would settle whether anything of the difference is the link's.

### S11. `Misconfigured`'s documentation names a NATS link's file

Left for the folds: "a NATS link that could not write them to its file" in `crates/ulo-rpc`.

### S12. The two-links test renamed

Decision 6. `two_links_write_files_of_their_own` became `two_loads_write_files_of_their_own`,
since two production links in one process share one file.

## Ledger text

For `FRAMEWORK_GAPS.md`, each appended under its entry.

Under F427:

*Appended 2026-10-18:* Fixed on `experiment/di-redesign` in `<commit>` by race 2b's test batch 41,
by the sixty-fifth response, which changes decision 139 to once per process while any link holds
the load. The read mechanism, probed at `460de797` (batch 41's `probes/p01-0.txt`): a `tls://`
client whose first call's 100 ms timeout ended during a 600 ms load, and whose second call waited,
ran two loads, the second call ending 711.1 ms after the first began. The load is now a `Load`
(`crates/ulo-rpc-nats/src/link.rs:1039`), a task spawned on the link's runtime's `spawn_blocking`
(`Load::start`, `:1062`) whose answer every holder awaits through a `watch` channel, so a call that
gives up drops its own wait and the load runs once. The process keeps a `Weak` to it (`PROCESS`,
`:953`, `Load::shared`, `:1050`): a link needing roots while another link or a connection holds the
load joins it, and once the last holder drops it the file is removed and the next link loads again.
A `tls://` link waits for the load (`Roots::wait`, `:990`); a `nats://` link waits for nothing
(`Roots::ready`, `:1005`): a server's `prepare` took 0.72 ms on `nats://` against 604.1 ms on
`tls://` with 600 ms roots (`probes/p05-0.txt`), where both waited (`probes/p04-0.txt`), and a
connection made once the load has ended is handed the file. Held by
`thirty_links_at_once_share_one_load_and_one_file`, one load and one file for thirty links preparing
at once, gone once they drop, then a second load; with a load per link it fails on 30 loads
(`broken/b01-0.txt`) and with the slot kept strong on the file left (`broken/b04-0.txt`); by
`a_call_that_gives_up_while_the_roots_load_leaves_the_load_running`, failing on a second load with
the load kept only once it answers (`broken/b03-0.txt`); and, behind `integration`, by
`plain::a_plain_link_connects_within_the_boot_budget_while_its_roots_load`, a `nats://` server and
client binding, connecting and carrying a request in 8.8 to 15.0 ms while the load blocked for
twice the 5 s boot budget, failing at 10.02 s with the link waiting (`broken/b02-0.txt`). The NATS
suite passed 20 of 20 runs on macOS, 6.19 to 11.38 s, one load a run of 0.20 to 0.31 s
(`probes/p03-*.txt`), where six runs with the link as `460de797` has it, in the same session, failed
9 to 50 scenarios each (`probes/p02-*.txt`). Its times were taken beside another batch's builds and
are not compared with batch 38's 5.7 s (batch 41's S10).

Under F423:

*Appended 2026-10-18:* Narrowed on `experiment/di-redesign` in `<commit>` by race 2b's test batch
41, F427's fix: a `nats://` link no longer waits for its roots, so a connection it makes before the
load ends is handed no file, and if its server upgrades that connection, async-nats reads the
platform's store on the thread polling it, at that connect and at each of its reconnects, since
async-nats fixes a connection's root files when it connects. A connection made once the load has
ended is handed the file. The `Nats` documentation and the crate's say so. Open for that
connection, by the sixty-fifth response. `over_tls::connecting_on_a_nats_url_to_a_server_requiring_tls_does_not_stall_the_thread`
now joins a process load finished before its links connect, one load where it counted four
(batch 41's S1, S5).

Under F422:

*Appended 2026-10-18:* By race 2b's test batch 41, F427's fix: the load is shared by every link in
the process, `tls://` or not, so a `tls://` link resolves the provider once, on the thread awaiting
its first need, before it joins the load (`Roots::wait`, `crates/ulo-rpc-nats/src/link.rs:990`),
where it resolved in the blocking task that loaded its roots. The refusal and its text are
unchanged, `a_tls_link_whose_provider_does_not_resolve_is_refused_naming_the_fix` still finds no
load, and `tests/two_providers.rs` passes (batch 41's S2). The upgraded `nats://` case stays open.

Under F417:

*Appended 2026-10-18:* By race 2b's test batch 41: the platform's roots load once per process while
any link holds them, no longer once per link (F427).

## Verification

Full output of every run is in the session scratchpad, `batch41/`: `probes/` and `probes/src/` for
the probes, `broken/` and `specs/` for the breaks, `suites/` for the NATS suite runs with
`summary.txt` and the container lists and roots listings before and after each, `verify/` for the
checks, `superseded/` for runs a later one replaced, and `scripts/`. `batch41/scripts/verify.sh`
ran the checks below in the mirror on the final crate, its tree hash `ea7c7d5d..` read before and
after (`verify/final-tree-hash.txt`, `verify/final-tree-hash-end.txt`). A first pass found one new
warning, `ProcessTurn::blocking` unused without `integration`; it is gated on the feature, and the
pass was run again (`superseded/verify1/`). The eight suite runs on the final tree followed it.

- **The NATS suite** with `integration`: 20 of 20 runs passed, 54 and 1 ignored each, batch 40's
  count (decision 8). The other RPC suites were not run: the batch changes no file outside
  `crates/ulo-rpc-nats`.
- **The NATS link's unit tests with `integration`,** one thread at a time: 20 passed, the three
  new beside the 17 `460de797` defines. **Without it:** 14 unit, where `460de797` runs 12 (p04's
  "12 filtered out" beside its probe); `conformance` 0, behind
  `integration`; `handle` 2; `two_providers` 3; 1 doctest ignored.
- **The unsafe check,** `python3 scripts/unsafe_allowed.py check`: "self-test: 6 planted lines
  refused, 2 valid lines accepted", 461 files scanned, 2 listed, 0 findings, exit 0.
- **The runtime-levels check,** `/usr/bin/python3 scripts/runtime_levels.py --locked`: 50 members,
  54 entries, 33 checked, 0 failures, exit 0.
- `cargo check --workspace --all-targets --all-features --locked` and `cargo check --workspace
  --all-targets --locked`, with the OpenSSL flags, and `cargo +1.88 check --workspace --all-targets
  --exclude ulo-http-salvo --exclude ulo-graphql-async-graphql --locked` in `target/msrv-1.88`: exit 0
  each, `ulo`'s 17 known warnings, none located outside `crates/ulo/src`.
- `RUSTDOCFLAGS=-Dwarnings cargo doc --no-deps --lib -p ulo-rpc-nats --locked`: exit 0.
- **Clippy:** `cargo +1.98.1 clippy -p ulo-rpc-nats --all-targets --all-features --no-deps --locked`:
  exit 0, no warning located in `crates/ulo-rpc-nats`.
- **The workspace test,** in the real tree with batch 42's edits, `cargo test --workspace
  --no-fail-fast --locked` with the OpenSSL flags: exit 0, 1,310 passed, 0 failed, 141 ignored, over
  203 lines starting `test result:`, where batch 40 had 1,300 and 141 over 203. The real tree then
  held batch 42's edits to `crates/ulo`, `crates/ulo-runtime-tokio/tests/app.rs` and
  `crates/ulo-rpc/tests/closed_app.rs` beside this batch's (`verify/real-tree-status-before-workspace-test.txt`);
  which tests the ten more are was not read.
- **The break runner's restore check:** `cmp` exits 1 on two files that differ
  (`verify/cmp-selftest.txt`), and the runner's tree hash reported `MISMATCH` when the tree changed
  during a run (decision 9).
- **The prose:** a scan for the banned register and for unfilled template braces, asserted first
  against a planted violation, over this log and every added line under `crates`, found none. The
  scan checks vocabulary only.
- **Containers and images:** at the batch's start twelve containers, the user's, every one exited,
  and 25 images (`batch41/docker_start.txt`). Every suite run's, probe's and break's container list
  after it matches its list before, and the end's lists match the start's, the same twelve
  containers by id and the same 25 images (`verify/docker-end.txt`). The batch's own containers
  were the suites' and tests' testcontainers, each removed by its scenario. The batch pulled no
  image, ran no `docker ... prune` and started, stopped or removed nothing of the user's.
