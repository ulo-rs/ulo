# Divergences: race 2b, tests batch 28, the runtime-levels check's path rule, tokio entering a runtime-neutral tree only through hyper, h2 or tokio-util, and a workspace member naming tokio-util directly only where the list gives the reason (S1)

The fifty-second response, signed off 2026-10-12, answers `runtime-f.md` and accepts its S4: in the
runtime-neutral set tokio may arrive only through hyper, h2 or tokio-util, and a direct dependency
from any of the workspace's crates fails. `scripts/runtime_levels.py` now reads each runtime-neutral
tree's edges as well as its features. Every package that depends on tokio directly must be hyper,
h2 or tokio-util, and a workspace member that depends on tokio-util directly must have a
`direct-tokio-util` line in `runtime-levels.txt` giving the reason. One member does today:
`ulo-http-conformance`, whose GOAWAY scenario drives h2's client over the host's `futures-io` stream
through tokio-util's `compat`, and the list names it. On the tree as it stands every entry holds,
and runtime-f's break 09, a direct `tokio` with `sync` on `ulo-listen-smol`, now fails.

Files changed: `scripts/runtime_levels.py` and `runtime-levels.txt`. The CI job is unchanged: it
runs the same command. Nothing in `FRAMEWORK_GAPS.md`: F389 is unused. The batch started from
`a8bb8611`; line numbers below are those of `a8bb8611`.

## What was built

### 1. The path rule

- **What it reads.** For an entry whose tree contains tokio, the script runs a second read,
  `cargo tree -p <crate> <flags> -e normal -i tokio --target=all --prefix depth`, with `--locked`
  when given. With `-e normal` every node is a package, and each line's parent is the package it
  depends on, so the children of `tokio` are its direct dependents and the children of `tokio-util`
  are tokio-util's. cargo prints a repeated node's children only at its first occurrence and every
  edge as a line, so the `(*)` de-duplication hides no direct dependent. The features read is
  unchanged.
- **Why a second read.** The features tree interleaves package and feature nodes, and a package
  that enables a tokio feature through a feature of its own sits under its own feature node rather
  than under tokio's. Runtime-f's `through` came from a depth-1-or-depth-2 heuristic over that tree;
  that heuristic is deleted, and `through` is now the exact set of tokio's direct dependents. Six
  entries take the second read today; three warm runs each took 3.9 to 5.2 s at `a8bb8611` and 4.8
  to 6.5 s now, the spread between runs as wide as the difference (`verify/timing-*.txt`).
- **The verdicts.** A runtime-neutral entry of either set fails on each package other than hyper,
  h2 or tokio-util that depends on tokio directly, naming it as a workspace member or as "no
  workspace member" (break 03); on each workspace member that depends on tokio-util directly with
  no `direct-tokio-util` line; and, as before, on a tokio feature outside its set. A runtime-free
  entry fails on any tokio, as before, its line now naming the exact direct dependents.
- **The `direct-tokio-util` line.** `<crate> direct-tokio-util <reason>`, the reason the rest of
  the line and required. The line is split on whitespace before `shlex` reads it, so an apostrophe
  in the reason is not a quote. A line with no reason, a line naming no member, and a second line for
  the same crate are errors (exit 2) before any tree is read. A line no runtime-neutral tree uses,
  the member depending on tokio-util directly in none of them, fails (S2); the check is skipped when
  a cargo error left a tree unread. The allowance covers the path and not the features: a listed
  member enabling tokio-util's `rt` fails on tokio's `rt` (break 06).
- **Output.** The entry line prints `through` as before and, where a listed member's direct
  tokio-util was read, `; tokio-util direct from <crate>, listed`. Every runtime-free and
  tokio-bound line prints what it printed at `a8bb8611` (`verify/final-run.txt` against
  `verify/before-run.txt`).
- **The docstring and the list header** each state the path rule in a sentence, and the docstring's
  verdict paragraph lists the new failures.

### 2. The tokio-util decision

`ulo-http-conformance` depends on `tokio-util` with `compat` directly
(`crates/ulo-http-conformance/Cargo.toml:53`): `cases/drain.rs:218` hands the host's `futures-io`
stream to `h2::client::handshake` as `stream.compat()`, because h2 reads tokio's I/O traits and
the scenario needs h2's client to see the GOAWAY frame, which hyper's client does not expose. The
other route is an adapter of the suite's own from `futures-io` to tokio's traits, which implements
`tokio::io::AsyncRead` and so names tokio directly, the dependency the rule bars. tokio-util is
allowed as a direct dependency where the list names it, with the reason, and the list names
`ulo-http-conformance` (S1). Without the line, both of its entries fail (`verify/no-allowance-run.txt`,
break 05).

No other runtime-neutral tree has a member depending on tokio-util directly. `ulo-http`'s
`tokio-io` feature brings `tokio` and `tokio-util` directly, and no runtime-neutral entry's normal
tree enables it: the axum, salvo, poem and rocket adapters, which are tokio-bound, enable it, and
`ulo-http-actix`, `ulo-ws` and `ulo-ws-hyper` in dev-dependencies alone. A runtime-neutral crate enabling it would fail on `ulo-http` depending on
tokio directly.

## The break runs

Each break went through the scratchpad's `scripts/brk.py`, runtime-f's harness unchanged: it
replaces each span (asserting it occurs once), updates `Cargo.lock` offline where the spec says
so, runs the script with `--locked`, writes every touched file and `Cargo.lock` back byte for
byte, and compares a hash over `git diff HEAD` of `crates`, `Cargo.toml`, `Cargo.lock`, `.github`,
`scripts`, `runtime-levels.txt` and `unsafe-allowed.txt` and every untracked file under them. Every
restore reported `ok` with the hash `e15a83ae..` (`verify/baseline-hash.txt`), the tree with this
batch's two files, and a final measure after all of them read the same (`verify/final-hash.txt`).
Specs in `specs/`, output in `broken/` with `summary.txt`.

| Break | Result |
| --- | --- |
| 01. runtime-f's break 09 re-run: `tokio` with `sync` named directly by `ulo-listen-smol` | exit 1, where runtime-f's was exit 0: `ulo-listen-smol` and the HTTP suite with `smol`, "takes tokio only through hyper, h2 or tokio-util, but workspace member ulo-listen-smol depends on tokio directly"; tokio's features `default`, `sync`, within the level |
| 02. `tokio` with `sync` named directly by `ulo-http-hyper` | exit 1: `ulo-http-hyper --no-default-features` and both HTTP suite entries, "workspace member ulo-http-hyper depends on tokio directly" |
| 03. `tokio-stream` with no default features, a third-party crate naming tokio with `sync`, on `ulo-listen-smol` | exit 1: `ulo-listen-smol` and the HTTP suite with `smol`, "tokio-stream, no workspace member, depends on tokio directly"; no feature outside the level |
| 04. `tokio-util` with `compat` named directly by `ulo-http-hyper` | exit 1: the same three entries, "workspace member ulo-http-hyper depends on tokio-util directly and runtime-levels.txt has no `direct-tokio-util` line for it"; tokio itself still through h2, hyper and tokio-util alone |
| 05. `ulo-http-conformance`'s `direct-tokio-util` line deleted | exit 1: both HTTP suite entries, "workspace member ulo-http-conformance depends on tokio-util directly and runtime-levels.txt has no `direct-tokio-util` line for it" |
| 06. `ulo-http-conformance`'s tokio-util with `compat` and `rt` | exit 1: both HTTP suite entries, "enables rt"; no path failure, the line naming the member |
| 07. a `direct-tokio-util` line for `ulo-hyper-serve` | exit 1: "`direct-tokio-util` for ulo-hyper-serve, but no runtime-neutral entry's tree has ulo-hyper-serve depending on tokio-util directly" |
| 08. a `direct-tokio-util` line with no reason | exit 2: "a `direct-tokio-util` line gives its reason after the keyword", no tree read |
| 09. a `direct-tokio-util` line for `ulo-planted` | exit 2: "names `ulo-planted`, which is no workspace member", no tree read |
| 10. `ulo-http-conformance`'s `direct-tokio-util` line twice | exit 2: "`ulo-http-conformance` has a second `direct-tokio-util` line" |
| 11. runtime-f's break 01 re-run: `tokio` with `sync` in `ulo-rpc`, runtime-free | exit 1: the same five entries as runtime-f, "runtime-free, but its tree contains tokio [sync] through ulo-rpc" |
| 12. runtime-f's break 05 re-run: `tokio` with `net` in `ulo-listen-smol` | exit 1: the same two entries and feature failures as runtime-f, each now with a second line, "workspace member ulo-listen-smol depends on tokio directly" |
| 13. runtime-f's break 08 re-run: `tokio` with `time` in `ulo-http-hyper` | exit 1: the same three entries and "enables time" as runtime-f, each with the path line naming `ulo-http-hyper` |
| 14. runtime-f's break 11 re-run: `ulo-cli`'s entry deleted | exit 1: "ulo-cli: a workspace member runtime-levels.txt does not classify", 53 entries |
| 15. runtime-f's break 14 re-run: `ulo-rpc-tcp-cor`, a misspelled crate | exit 2: "`ulo-rpc-tcp-cor` is no workspace member" beside `ulo-rpc-tcp-core` unclassified, no tree read |
| the restored tree | exit 0, every entry `ok` |

Runtime-f's feature breaks 04 to 08 each planted `tokio` on a workspace member directly, so each now
fails twice, on the feature and on the path; 12 and 13 show it. Their verdicts are unchanged. Breaks
02 and 13 need no new lock entry, `ulo-http-hyper` already naming tokio as a dev-dependency; break 03
resolves `tokio-stream` 0.1.19 offline, already in `Cargo.lock`.

## Left for the transports DESIGN fold

Line numbers are those of `a8bb8611`.

- §8's levels paragraph (line 1312): the runtime-neutral level "depends on tokio only for its
  runtime-free `sync` primitives, which hyper brings, plus `io-util` and `bytes` on the HTTP/2 path,
  which `h2` brings" is now checked as written. The check holds both the features and the path:
  tokio enters through hyper, h2 or tokio-util, any other package depending on tokio directly fails
  the entry, and a workspace member depending on tokio-util directly is allowed only where
  `runtime-levels.txt` names it with the reason.
- The paragraph at line 1324, "Not built: the CI check .. fails .. the runtime-neutral set on any
  tokio feature beyond `sync`, `io-util` and `bytes`": runtime-f's fold already records the check as
  built; the rule it states adds the path.
- §8's table, the runtime-neutral row (line 1319): "the suite's client is hyper's and `h2`'s over
  the harness's stream, `h2` through `tokio-util`'s `compat`" is the one listed direct tokio-util,
  and the row can say the list names it, with the reason: the other adapter would name tokio.
- Decision 104's last sentence (line 1604) and decision 99's last (line 1599), "any tokio feature
  beyond `sync`, `io-util` and `bytes` fails the second": the rule is also the path, as above.
- §8's sentence that "the design's own crates at this level use none of `tokio::sync`, `async-channel`
  and its kin always being a choice" (line 1312) speaks of the runtime-free level; answer 4's
  principle, as the fifty-second response's S4 states it, now holds at the runtime-neutral level by
  check: no workspace member there names tokio.

## Needs sign-off

Numbered from S1 for this batch.

### S1. tokio-util is allowed as a direct dependency where the list names it

The tokio-util decision. `ulo-http-conformance` keeps its direct `tokio-util` with `compat`, named by
a `direct-tokio-util` line whose reason is required. The alternative, routing the GOAWAY scenario's
h2 client through an adapter of the suite's own, implements tokio's I/O traits and names tokio
directly, which the rule fails; dropping h2's raw client loses the scenario, hyper's client not
exposing GOAWAY.

### S2. A `direct-tokio-util` line no runtime-neutral tree uses fails

Break 07. An allowance outlives its reason only until the next CI run: when the member stops naming
tokio-util, the line must go. The alternative is to accept an unused line, which leaves a member free
to take tokio-util directly later under a reason written for another use.

### S3. The path rule judges every package, not only workspace members

Break 03. A third-party crate that depends on tokio directly fails a runtime-neutral entry, naming the
crate, as the brief's "every package that depends on tokio directly must be one of hyper, h2 or
tokio-util" reads. The fifty-second response words the failure as a direct dependency "from any of
our crates"; held to members alone, a dependency of ours that brings tokio with `sync` would pass
the path rule and be checked on features only, as runtime-f did for everything.

## Verification

Full output of every run is in the session scratchpad, `batch28/`: `verify/` for the runs below,
`probe/` for the inverted trees read before the change, the HTTP suite's at both feature sets, the
HTTP backend's without `tokio` and the smol listener's, `specs/`
and `broken/` for the breaks, `scripts/` for `brk.py`, `orig/` for the script and list at
`a8bb8611`.

- **The script on the tree,** `python3 scripts/runtime_levels.py --locked` under macOS's Python
  3.9.6, `/usr/bin/python3`: exit 0, 50 members, 54 entries, 33 read, no failure and no error
  (`verify/final-run.txt`). Before the list's
  `direct-tokio-util` line was added, the new script failed both HTTP suite entries on it, exit 1
  (`verify/no-allowance-run.txt`).
- **The unsafe check** still passes: `python3 scripts/unsafe_allowed.py check`, exit 0, 454 files
  scanned, 3 listed, no finding (`verify/unsafe-check.txt`).
- **The CI YAML** is unchanged; `git diff HEAD -- .github` is empty.
- **No `cargo check` and no suites** ran: no Rust code or manifest changed, and every manifest the
  breaks touched was restored byte for byte, the hash reading the same.
- **Containers:** none started. No permission check refused an action.
