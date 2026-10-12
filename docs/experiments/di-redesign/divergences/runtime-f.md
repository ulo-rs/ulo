# Divergences: runtime stage f, the CI check holding every workspace member to its level of runtime independence, no tokio in a runtime-free crate's tree, tokio's `sync` alone in a runtime-neutral one and `io-util` and `bytes` on the HTTP/2 path, and no member left unclassified

The forty-eighth response, signed off 2026-10-11, answers `runtime-e2.md` and makes the two levels
of transports DESIGN §8 a CI check. `scripts/runtime_levels.py` reads each entry of a new list,
`runtime-levels.txt`, through `cargo tree -p <crate> <flags> -e normal,features -i tokio
--target=all`, fails a runtime-free entry whose tree contains tokio, fails a runtime-neutral entry
whose tree enables a tokio feature outside its level's set, and fails on a workspace member the list
does not name. A new `runtime levels` CI job runs it with `--locked`. On the tree as committed every
entry holds: 27 runtime-free entries print no tokio, the six runtime-neutral ones print `default`
and `sync`, with `io-util` and `bytes` on the two HTTP/2 entries, and 21 tokio-bound entries are
listed and not read. No crate violates its level, and no code changed.

Files changed: `.github/workflows/ci.yml` (the `runtime-levels` job), and two new files,
`runtime-levels.txt` and `scripts/runtime_levels.py`. Nothing in `FRAMEWORK_GAPS.md`: F389 is
unused. The stage started from `84e35eff`; `3e1dca09`, committed by another agent during the
build, touches the two DESIGN files alone, so the tree is `3e1dca09` plus this stage. Line numbers
below are those of `3e1dca09`.

## What was built

### 1. The script

`python3 scripts/runtime_levels.py [--locked]`, standard-library Python that runs on macOS's
Python 3.9 and a stock runner, in the form of `scripts/unsafe_allowed.py`.

- **What it reads.** For each entry not tokio-bound it runs `cargo tree -p <crate> <flags> -e
  normal,features -i tokio --target=all --prefix depth`, with `--locked` when given, and collects
  every `tokio feature "<name>"` node, each a feature enabled in that tree, wherever it appears, so
  cargo's `(*)` de-duplication hides none. It also names the packages that depend on tokio
  directly, at depth 1 or under a tokio feature node at depth 2, and prints them as `through`; the
  verdict does not read them (S4).
- **Absence, two ways.** cargo answers a tree without tokio with "package ID specification `tokio`
  did not match any packages" and exit 101 when tokio is not in the resolved graph at all, and with
  "nothing to print" and exit 0 when it is there by a dev- or build-dependency edge alone, as on the
  four hubs whose tests run on tokio. Both pass and the verdict names which. Any other non-zero exit
  is an error carrying cargo's stderr, a zero exit with empty output and no "nothing to print" is an
  error, and so is a tree whose root is not tokio.
- **The verdicts.** A runtime-free entry fails on a tokio root at all, features or none. A
  runtime-neutral entry fails on any feature outside `default` and `sync`; a `runtime-neutral+http2`
  entry on any outside `default`, `sync`, `io-util` and `bytes`. tokio's implicit features count:
  a planted `net` reads as `net`, `libc`, `mio`, `socket2` and `windows-sys`, and each is outside
  both sets.
- **Classification.** The members come from `cargo metadata --no-deps`; a member with no entry is a
  failure. An entry naming no member is an error before any tree is read: given `-i`, cargo 1.98.1
  widens an unknown `-p` to the whole workspace and exits 0 (`probe/` in the scratchpad), so a
  misspelled crate would read every member's tokio rather than be refused.
- **The list's grammar.** `<crate> <level> [flags]`, the flags limited to `--features`, `-F`,
  `--all-features` and `--no-default-features`, so an entry chooses a crate's features and never
  what the tree reads; an unknown level, a flag outside those, or the same crate and flags twice is
  an error. A crate may have several entries, one per feature set.
- **Exit status.** 0 when every entry holds; 1 on a failure or an unclassified member; 2 on a cargo
  error or a malformed list, so a cargo error is never read as a verdict. Each entry prints one line
  with `ok`, `FAIL`, `ERROR` or `skip`, its level, crate and flags, and the features found; the
  failures and errors follow, then a count.
- **`--target=all`.** cargo reads the host's dependencies by default, so a tokio that one platform's
  dependencies bring is invisible from another. Read across every platform, the verdict is the same
  on the Linux runner and on macOS. Today it adds `windows-sys` to the features of tokio-bound crates
  alone, and every checked entry reads the same with and without it (`probe/survey-*.txt`). See S1.

### 2. The CI job

`runtime levels`, after `check`: checkout, stable toolchain, `Swatinem/rust-cache@v2`, then
`python3 scripts/runtime_levels.py --locked`. A job of its own rather than a step in `check`: a crate
leaving its level fails under the rule's name, and a failing step in `check` would skip the compile
behind it or run after it and report under the compile's name. The cost is a runner and the
registry: `cargo tree` compiles nothing but downloads every crate in the lock to read its manifest,
92 s with an empty cargo home (`verify/cold-cargo-home-run.txt`), 3.4 s warm; the cache holds the
registry, and the job has no target directory to cache.

### 3. The levels where §8 names no crate

Each member was read at the flags its entry gives before it was listed (`probe/survey-*.txt`).

- **Tokio-free crates outside §8's table are runtime-free**: the five macro crates
  (`ulo-macros`, `ulo-transport-macros`, `ulo-http-macros`, `ulo-ws-macros`, `ulo-rpc-macros`),
  `ulo-handler-codegen`, `ulo-grpc-macros`, `ulo-build`, and the two GraphQL engines. The macro
  crates are in the core's and the hubs' trees already; listing them names the crate that would
  bring tokio. `ulo-grpc-macros` and `ulo-build` are used by gRPC alone, which is tokio-bound. See S2.
- **Tokio-bound outside §8's table**: `ulo-cli`, through `watchexec`; `ulo-codegen-tests` and
  `ulo-macro-lints`, through `ulo-grpc`.
- **`--all-features` for a runtime-free crate** unless a feature of its own brings tokio, so a feature
  added later is read without the list changing. The two exceptions are read at default features:
  `ulo-http`, whose `tokio-io` brings tokio with `default` and `sync` for `Upgraded`'s tokio traits,
  and `ulo-rpc-conformance`, whose `tokio` brings the tokio harness; it is read a second time with
  `--features smol`, as §8's table names. See S3.
- **The HTTP suite is read twice**, at default features and with `--features smol`, the second being
  what the `smol` job runs. Its `tokio` and `rocket` features bring their hosts' runtimes.
- **Two runtime-neutral sets.** `ulo-hyper-serve`, `ulo-listen-smol` and `ulo-ws-hyper` without
  `tokio` print `default` and `sync` and are held to them; `ulo-http-hyper` without `tokio` and the
  HTTP suite serve or speak HTTP/2 and are held to `default`, `sync`, `io-util` and `bytes`. See S5.
- **`ulo-http-hyper` and `ulo-ws-hyper` have a second entry**, tokio-bound, for their default
  `tokio` feature, so the list records both standings §8 gives them.

## The list

`runtime-levels.txt`, each entry headed by a comment stating why the crate is at its level. The
tree each entry printed on the committed tree (`verify/final-run.txt`):

| Crate | Level | Flags | Tree |
| --- | --- | --- | --- |
| `ulo` | runtime-free | `--all-features` | did not match |
| `ulo-macros` | runtime-free | `--all-features` | did not match |
| `ulo-handler-codegen` | runtime-free | `--all-features` | did not match |
| `ulo-transport` | runtime-free | `--all-features` | did not match |
| `ulo-transport-macros` | runtime-free | `--all-features` | did not match |
| `ulo-net` | runtime-free | `--all-features` | did not match |
| `ulo-smol` | runtime-free | `--all-features` | did not match |
| `ulo-runtime-conformance` | runtime-free | `--all-features` | did not match |
| `ulo-test-certs` | runtime-free | `--all-features` | did not match |
| `ulo-http` | runtime-free | default | nothing to print |
| `ulo-http-macros` | runtime-free | `--all-features` | did not match |
| `ulo-ws` | runtime-free | `--all-features` | nothing to print |
| `ulo-ws-macros` | runtime-free | `--all-features` | did not match |
| `ulo-ws-conformance` | runtime-free | `--all-features` | did not match |
| `ulo-rpc` | runtime-free | `--all-features` | nothing to print |
| `ulo-rpc-macros` | runtime-free | `--all-features` | did not match |
| `ulo-rpc-tcp-core` | runtime-free | `--all-features` | did not match |
| `ulo-rpc-tcp-smol` | runtime-free | `--all-features` | did not match |
| `ulo-rpc-conformance` | runtime-free | default | did not match |
| `ulo-rpc-conformance` | runtime-free | `--features smol` | did not match |
| `ulo-graphql` | runtime-free | `--all-features` | did not match |
| `ulo-graphql-http` | runtime-free | `--all-features` | did not match |
| `ulo-graphql-ws` | runtime-free | `--all-features` | nothing to print |
| `ulo-graphql-async-graphql` | runtime-free | `--all-features` | did not match |
| `ulo-graphql-juniper` | runtime-free | `--all-features` | did not match |
| `ulo-grpc-macros` | runtime-free | `--all-features` | did not match |
| `ulo-build` | runtime-free | `--all-features` | did not match |
| `ulo-hyper-serve` | runtime-neutral | `--all-features` | `default`, `sync` through hyper |
| `ulo-listen-smol` | runtime-neutral | `--all-features` | `default`, `sync` through hyper |
| `ulo-ws-hyper` | runtime-neutral | `--no-default-features` | `default`, `sync` through hyper |
| `ulo-http-hyper` | runtime-neutral+http2 | `--no-default-features` | `bytes`, `default`, `io-util`, `sync` through h2, hyper, tokio-util |
| `ulo-http-conformance` | runtime-neutral+http2 | default | the same |
| `ulo-http-conformance` | runtime-neutral+http2 | `--features smol` | the same |
| `ulo-ws-hyper`, `ulo-http-hyper` | tokio-bound | default | not read |
| `ulo-tokio`, `ulo-listen-tokio` | tokio-bound | default | not read |
| `ulo-http-axum`, `ulo-http-actix`, `ulo-http-salvo`, `ulo-http-poem`, `ulo-http-rocket` | tokio-bound | default | not read |
| `ulo-rpc-tcp`, `ulo-rpc-udp`, `ulo-rpc-nats`, `ulo-rpc-redis`, `ulo-rpc-rabbitmq`, `ulo-rpc-mqtt`, `ulo-rpc-kafka` | tokio-bound | default | not read |
| `ulo-ws-redis`, `ulo-grpc` | tokio-bound | default | not read |
| `ulo-codegen-tests`, `ulo-macro-lints`, `ulo-cli` | tokio-bound | default | not read |

50 members, 54 entries, 33 read.

## The break runs

Each break went through the scratchpad's `scripts/brk.py`: it replaces each span (asserting it occurs
once) or creates the planted files, updates `Cargo.lock` offline where the spec says so, runs the
script with `--locked`, writes every touched file and `Cargo.lock` back byte for byte, deletes what it
created, and compares a hash over `git diff HEAD` of `crates`, `Cargo.toml`, `Cargo.lock`,
`.github`, `scripts`, `runtime-levels.txt` and `unsafe-allowed.txt` and every untracked file under
them. Every restore reported `ok`, the hash `c7fed0b6..` (`verify/baseline-hash.txt`), and a final
measure after all of them read the same. Specs in `specs/`, output in `broken/` with `summary.txt`.

| Break | Result |
| --- | --- |
| 01. `tokio` with `sync` in `ulo-rpc`'s dependencies, a hub carrying tokio for `sync` alone | exit 1: `ulo-rpc`, `ulo-rpc-tcp-core`, `ulo-rpc-tcp-smol` and `ulo-rpc-conformance` at both feature sets failed, "runtime-free, but its tree contains tokio [sync] through ulo-rpc" |
| 02. `tokio` with no feature in the core's dependencies | exit 1: every runtime-free entry that depends on `ulo` failed, "tokio [] through ulo"; the macro crates, `ulo-handler-codegen`, `ulo-test-certs`, `ulo-grpc-macros` and `ulo-build` held, and the six runtime-neutral entries passed, a featureless tokio adding no feature (S4) |
| 03. `tokio` with `rt` in `ulo-test-certs` | exit 1: that entry alone, "tokio [rt] through ulo-test-certs" |
| 04. `tokio` with `rt` in `ulo-hyper-serve` | exit 1: all six runtime-neutral entries, each "allows tokio's .., but its tree enables rt" |
| 05. `tokio` with `net` in `ulo-listen-smol` | exit 1: `ulo-listen-smol` and the HTTP suite with `smol`, "enables libc, mio, net, socket2, windows-sys" |
| 06. `tokio` with `io-util` in `ulo-listen-smol` | exit 1: `ulo-listen-smol`, "allows tokio's default, sync, but its tree enables bytes, io-util"; the HTTP/2 allowance does not reach a crate off the HTTP/2 path |
| 07. `tokio` with `net` in `ulo-http-hyper` | exit 1: `ulo-http-hyper` without `tokio` and both HTTP suite entries, "allows tokio's bytes, default, io-util, sync, but its tree enables libc, mio, net, socket2, windows-sys" |
| 08. `tokio` with `time` in `ulo-http-hyper` | exit 1: the same three entries, "enables time" |
| 09. control: `tokio` with `sync` named directly by `ulo-listen-smol` | exit 0: the feature set is within the level, and the rule reads no path (S4) |
| 10. a new member `crates/ulo-planted`, added to `members` | exit 1: "ulo-planted: a workspace member runtime-levels.txt does not classify", 51 members |
| 11. `ulo-cli`'s entry deleted | exit 1: "ulo-cli: a workspace member runtime-levels.txt does not classify" |
| 12. `tokio` added to the core's manifest, `Cargo.lock` not updated | exit 2: all 33 read entries `ERROR`, each with cargo's "cannot update the lock file .. because --locked was passed", no `ok` and no `FAIL` |
| 13. `--features smoll` on an entry | exit 2: "the package 'ulo-rpc-conformance' does not contain this feature: smoll", every other entry read |
| 14. `ulo-rpc-tcp-cor`, a misspelled crate | exit 2: "`ulo-rpc-tcp-cor` is no workspace member", beside "ulo-rpc-tcp-core: a workspace member runtime-levels.txt does not classify", no tree read |
| 15. `-p ulo` as an entry's flags | exit 2: "flags may only select features" |
| 16. `runtime-fre` as a level | exit 2: "`runtime-fre` is no level" |
| the restored tree | exit 0, every entry `ok` |

Break 12 ran first in the working tree, where it printed `ERROR` for the first eleven entries and
then `FAIL` with tokio through `ulo` for the rest: `Cargo.lock` had been rewritten to include the
planted edge part-way through the run, which `--locked` does not do. rust-analyzer was running
against the tree and re-resolves the lock when a manifest changes, which is the likely writer; the
output is kept under `broken/superseded/`. The break was then run in `wt/`, a copy of `84e35eff`
from `git archive` with the script and the list added, which no editor watches, with the result
above, the two touched files' hashes the same before and after. A first break 12, planting on
`ulo-rpc`, did not fail `--locked` at all: `ulo-rpc` already depends on tokio as a dev-dependency, and
`Cargo.lock` records that edge whatever its kind; it is kept under `broken/superseded/` too. Breaks
01, 07 and 08 needed no lock update for the same reason.

## Left for the transports DESIGN fold

Line numbers are those of `3e1dca09`.

- §8's levels paragraph (line 1312) and its table (lines 1314-1322): the closing "Not built: the CI
  check holding each crate to its level .. (F383)" in the paragraph at line 1324 is built. The check
  is `scripts/runtime_levels.py`, run by the `runtime levels` job, over `runtime-levels.txt`, which
  classifies every member; it reads `cargo tree -p <crate> <flags> -e normal,features -i tokio
  --target=all`, where line 1312 gives `-e normal -i tokio -f "{p} {f}"` as the read, and it splits
  the runtime-neutral level in two, `sync` for `fw-hyper-serve`, `fw-listen-smol` and `fw-ws-hyper`,
  and `sync`, `io-util` and `bytes` for `fw-http-hyper` and `fw-http-conformance`.
- The table's crates: `fw-graphql-async-graphql` and `fw-graphql-juniper`, the five macro crates,
  `fw-handler-codegen`, `fw-grpc-macros` and `fw-build` are runtime-free and checked; `fw-cli`,
  `fw-codegen-tests` and `fw-macro-lints` are tokio-bound; the HTTP suite is runtime-neutral with
  `smol` as at its default features. None of these is in the table.
- "Every plug-point trait and type" at the runtime-free level, in §0's principle 5 (line 13), line
  1312, and decision 104 (line 1604), with decision 99's title (line 1599): `fw-hyper-serve` holds the
  `Listener` plug point (§3.7) and is runtime-neutral, so the clause does not hold of `Listener`, as
  F383 records of every crate on hyper. See S6.
- Decisions 99 and 104: "the forbidden-dependency check, not built" (line 1599) is built, its rule
  as 104 states it with the HTTP/2 features held to the HTTP/2 entries.
- §0's principle 5 or §8: the CI job list, `runtime levels` beside `check`.

## Needs sign-off

Numbered from S1 for this stage.

### S1. The tree is read across every platform, `--target=all`

What the script reads, decision 1. The verdict is the same on the Linux runner and on macOS, and a
tokio that a crate brings only under one platform's `cfg` fails everywhere. Today the two reads
differ only in `windows-sys` among tokio-bound crates' features. The alternative is the host's tree,
the command as briefed, which on the runner misses a tokio that only macOS or Windows dependencies
bring.

### S2. Tokio-free crates §8 does not name are held at runtime-free

Decision 3. The five macro crates, `ulo-handler-codegen`, `ulo-grpc-macros`, `ulo-build` and the two
GraphQL engines are listed runtime-free, so any of them gaining tokio fails CI until its line moves.
The alternative for `ulo-grpc-macros` and `ulo-build`, which only gRPC uses, is tokio-bound, which
leaves them unchecked.

### S3. Runtime-free crates are read with `--all-features`

Decision 3. A feature added to a runtime-free crate is read without the list changing, and one that
brings tokio fails until the entry narrows its flags. `ulo-http` and `ulo-rpc-conformance` are read at
default features, the second also with `smol`. The alternative is default features everywhere, as
§8 words the suites' level, which leaves a new feature unread.

### S4. The rule reads features, not the path tokio enters by

Decision 1 and break 09. A runtime-neutral crate naming tokio itself with `sync` passes, though the
level's wording is tokio "which hyper brings". The alternative is F383's candidate: also fail a
runtime-neutral entry where a package other than hyper, h2 or tokio-util depends on tokio directly,
which the script already prints as `through` and does not judge.

### S5. Two runtime-neutral sets, `io-util` and `bytes` only on the HTTP/2 entries

Decision 3 and break 06. `ulo-hyper-serve`, `ulo-listen-smol` and `ulo-ws-hyper` fail on `io-util`,
which they print none of today. The alternative is one set for the whole level, `default`, `sync`,
`io-util` and `bytes`, as §8 and decision 104 word it.

### S6. `Listener` is a plug point in a runtime-neutral crate

The fold's third item. §8 and decision 104 place every plug-point trait and type at runtime-free and
`fw-hyper-serve` at runtime-neutral; `Listener` lives in `fw-hyper-serve`, its `Io` bounded by hyper's
I/O traits, so an alternative listener has tokio with `sync` in its tree. The check holds the crates
where they are. The choices are the wording, "every plug-point trait and type but `Listener`, which
names hyper's traits", or the hyper-free `fw-hyper-serve` decision 99 does not build.

## Verification

The breaks ran on the tree hashed `c7fed0b6..` by `brk.py`'s measure. Two comments in
`runtime-levels.txt`, the GraphQL engines', were reworded after them and nothing else; every run
below is on the result, hashed `afe1252f..` (`verify/final-tree-hash.txt`). Full output of every run
is in the session scratchpad, `runtime-f/`: `probe/` for the surveys of
every member's tree and cargo's answers to an unknown `-p`, `verify/` for the runs below, `specs/` and
`broken/` for the breaks, `scripts/` for `brk.py`, and `wt/`, the scratch copy break 12 ran in.

- **The script on the committed tree,** `python3 scripts/runtime_levels.py --locked` under macOS's
  Python 3.9.6, `/usr/bin/python3`: exit 0, 50 members, 54 entries, 33 read, no failure and no error
  (`verify/final-run.txt`). The same in `wt/` with an empty cargo home: exit 0, 92 s
  (`verify/cold-cargo-home-run.txt`).
- **The job's steps locally:** the run above is its one command; the checkout, toolchain and cache
  steps are the other jobs'.
- **The YAML:** `ci.yml` parses with PyYAML, the job reading as four steps, and a malformed file
  fed to the same call raises (`verify/yaml-parse.txt`). `actionlint` is not installed here.
- **The unsafe check** still passes, the new script being no `.rs` file (`verify/unsafe-check.txt`).
- `cargo check --workspace --all-targets --all-features --locked` with
  `CFLAGS=-I/opt/homebrew/opt/openssl/include LDFLAGS=-L/opt/homebrew/opt/openssl/lib`:
  exit 0 in 17 min 21 s, most of it waiting on the build-directory lock rust-analyzer held, with
  `ulo`'s 17 known warnings and no warning located outside `crates/ulo/src`
  (`verify/cargo-check-all-features.txt`).
- **No suites** ran: no code changed.
- **Containers:** none started. No permission check refused an action.
