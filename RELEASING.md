# Releasing

Every publishable crate in this workspace carries one version, `[workspace.package] version` in the
root `Cargo.toml`, and every internal dependency a published crate ships pins it exactly
(`version = "=<version>"` beside its `path`). `release-plz.toml` describes the release in two
tiers. release-plz runs in check mode only: CI's `release check` job exercises the configuration
on every pull request and every push to `master`, and nothing publishes, tags or opens a pull
request from CI.

## The two tiers

- **The core** is every `[[package]]` in `release-plz.toml` with `version_group = "core"`. It
  releases at one version under one tag, `v<version>`, carried by `ulo`. A crate belongs to it when
  it owns a doc-hidden module (macro ABI, which carries no semver promise), is a proc-macro crate,
  writes `::ulo..::` paths into generated code (`ulo-build`), or is a dependency of a core crate.
- **A leaf** is every other publishable crate, tagged `<package>-v<version>`. A leaf inherits the
  workspace version as the core does, so a release moves every publishable crate. Until the first
  release that is not a pre-release, a leaf at another version fails the lockstep step.

`ulo-graphql` and `ulo-graphql-ws` are leaves by both conditions the rule reads. No core crate
depends on either, `=` or otherwise: in `cargo metadata` their dependents are `ulo-graphql-http`,
`ulo-graphql-async-graphql`, `ulo-graphql-juniper`, `ulo-graphql-ws` itself on `ulo-graphql`, and
the GraphQL examples, each a leaf or unpublished. Neither declares a `#[doc(hidden)]` item, and no
core crate's source names `ulo_graphql` or `ulo_graphql_ws`. `python3 scripts/release_check.py
lockstep` derives the same group.

`ulo-conformance-support` and `ulo-rpc-conformance`, the RPC conformance suite and its harness,
are leaves by the same two conditions. Neither owns a doc-hidden module: `conformance_suite!` and
`startup_failed!` expand to `Slots` and `startup_failed`, public items of the support crate that
the suite re-exports, so the code a suite stamps reaches no hidden path. No core crate depends on
either through a normal or build dependency: `ulo-http`, `ulo-ws` and `ulo-server-hyper` take the
support crate as a dev-dependency, which Cargo strips when publishing, and the suite's dependents
are the RPC links, `ulo-ws-redis` and the community-link fixture, each through a dev-dependency.

`ulo-runtime-conformance`, the runtime conformance suite, is a leaf by the same two conditions. It
owns no doc-hidden module: `runtime_suite!` expands to public items of the suite, reached through
`$crate`, and to items it writes beside the harness. Its dependencies are `ulo`, `ulo-transport`, `futures`
and `tracing`, and its dependents, `ulo-runtime-tokio` and `ulo-runtime-smol`, take it as a
dev-dependency, which Cargo strips when publishing; the suite takes `ulo-runtime-smol` as one in
turn, for the README's first run.

`ulo-http-conformance`, the HTTP conformance suite, is a leaf by the same two conditions. It owns no
doc-hidden module: `http_conformance_suite!` expands to `Slots`, a public item of the support crate
that the suite re-exports, and to the suite's own public items, reached through `$crate`. Its
dependencies are `ulo`, `ulo-http`, `ulo-http-hyper` (the reference host), `ulo-server-hyper`,
`ulo-transport` and the support crate, and behind its `tokio` and `smol` features, off by default,
`ulo-runtime-tokio` and `ulo-listener-tokio` or `ulo-runtime-smol` and `ulo-listener-smol`.
`ulo-http-hyper`, the five embedding host crates, `ulo-ws` and `ulo-ws-hyper` take it as a
dev-dependency, which Cargo strips when publishing, and the suite takes itself as one for the
README's first run. The community-host fixture takes it through a dev-dependency too.

## What `release check` checks

The job checks out the last release, the nearest tag matching `v[0-9]*` among the ancestors of the
commit under test, and runs `python3 scripts/release_check.py check <that checkout>`, which stops
at the first step that fails:

1. **Lockstep.** The core group is exactly the crates the rule above selects, and every member has
   `ulo`'s version, as has every leaf while that version is a pre-release.
2. **release-plz.** `release-plz update` runs in a copy of the tree, comparing each crate with the
   last release's manifests rather than with crates.io, and with no token. It must report on every
   publishable crate, and the versions it writes must keep the lockstep rule.
3. **Semver.** `cargo semver-checks` compares every publishable library crate with the last
   release. A version that Cargo's caret requirement on the released one still accepts, an
   unchanged version or a pre-release step among them, is held to a compatible release, and every
   break must be accepted in `ci/semver-breaks.txt`. Any other version is a major release, which
   accepts its breaks without an entry; it is still run as a compatible release, which lists them
   in the log, since a major release type skips every lint. Every crate must report more than zero
   checks run, so a run that skipped its lints fails rather than passing empty. Proc-macro crates
   and `ulo-cli`, a binary, have no library API to compare and are named apart in the job's last
   line, and so is a crate the last release does not publish, which has no release to compare
   with: one added since, or one made publishable since.

cargo-semver-checks has no lint for a changed type: a parameter or return type that changes, or a
tuple struct's field that stops being public, passes step 3. A change making such a break adds a
comment line to `ci/semver-breaks.txt` in the form `# unchecked <package> <item>  # reason`, which
the script skips. A change to what an unchanged call does, a default that refuses what it admitted
or a status a request is answered with, is a break of the same kind and gets the same line. The pull request that cuts a release lists those lines in the release's notes
beside the `break` entries; the first change after the tag replaces both.

## Reading a failure

Each finding starts a line on stderr, prefixed with its step; a finding about a break lists the
items it counts on the lines below it. A message with no prefix and exit status 2 is a usage
error or a malformed line in `ci/semver-breaks.txt`. One prefixed `self-test:` means the script no
longer refuses a violation it plants, a fault in the script rather than in the tree.

| Finding | What to do |
| --- | --- |
| `lockstep: X ..., so it belongs in group core` | Add a `[[package]]` for `X` with `version_group = "core"` and `git_tag_enable = false`. |
| `lockstep: X is in group core but ...` | Remove its `[[package]]`: nothing makes it part of the core. |
| `lockstep: X is at V, the core at W` or `X is at V, and every crate is at the core's W ...` | Give `X` the workspace version again. |
| `release-plz: update exited N` | The log above it carries release-plz's error, such as a key `release-plz.toml` may not hold. |
| `release-plz: release-plz did not version X` or `determined no version` | release-plz passed over `X`, or over every crate: its log above shows what it printed instead. |
| `release-plz: after the update, ...` | The versions release-plz would write break the lockstep rule; the rest of the line is a `lockstep:` finding. |
| `semver: X breaks L and ci/semver-breaks.txt does not accept it` | Restore the item, or, when the break is wanted, add the `break` line the finding prints, with its reason. |
| `semver: X breaks L in N items and ci/semver-breaks.txt accepts M` | The finding lists the items now reported; set the count to their number. |
| `semver: ci/semver-breaks.txt accepts X L, which this run did not report` | Remove the entry. |
| `semver: X is at V, behind the release's W` | Give `X` a version no lower than the release's. |
| `semver: X ran no checks` or `X was not checked` | The run proved nothing for `X`: the log shows the rustdoc build that failed or the lints that were skipped. |
| `semver: X is a major release, whose version accepts its breaks; remove X L ...` | Remove the entry. |

## Running it locally

The job pins release-plz `0.3.174`, cargo-semver-checks `0.51.0` and Rust `1.98.1`; the same
versions reproduce it.

```sh
cargo install --locked --root ~/.local/release-tools release-plz --version 0.3.174
cargo install --locked --root ~/.local/release-tools cargo-semver-checks --version 0.51.0
git worktree add --detach ../ulo-last-release "$(git describe --tags --abbrev=0 --match 'v[0-9]*')"
PATH="$HOME/.local/release-tools/bin:$PATH" RUSTUP_TOOLCHAIN=1.98.1 \
    python3 scripts/release_check.py check ../ulo-last-release
```

The check reads the working tree, uncommitted changes included, and builds into
`target/release-check` unless `CARGO_TARGET_DIR` is set. `ulo-rpc-kafka` does not build without
OpenSSL headers, which its `tls` feature compiles against; `--skip ulo-rpc-kafka` leaves it out of
release-plz's semver check and out of step 3. On macOS, Homebrew's `openssl@3` provides them once
`CPPFLAGS=-I"$(brew --prefix openssl@3)/include"` and `LDFLAGS=-L"$(brew --prefix openssl@3)/lib"`
are set; librdkafka's configure script does not read `OPENSSL_DIR`.
`python3 scripts/release_check.py lockstep` runs step 1 alone, in seconds. The worktree stays
registered until `git worktree remove ../ulo-last-release`, which the next release's checkout
needs first.

## Cutting a release

One pull request moves `[workspace.package] version` and every `=` pin to the new version,
regenerates `Cargo.lock` for the workspace members, and updates the Install sections of `README.md`,
`crates/ulo/README.md`, `crates/ulo-rpc-conformance/README.md`,
`crates/ulo-runtime-conformance/README.md` and `crates/ulo-http-conformance/README.md`, which
`scripts/check_readme_install.py` holds to the workspace version.
While that version is a pre-release the Install sections pin each crate `=<version>`, and the
script refuses any other operator: a caret on a pre-release also takes a later pre-release of the
same version, which may break the first.
The release is tagged `v<version>`. From that tag on, `release check` compares with the new release
and the `against` line in `ci/semver-breaks.txt` no longer matches it, so its entries are not applied.
The first change that breaks a public item replaces the file with entries for the breaks since the
new release and sets `against` to it.

## Publishing

`scripts/publish.py` publishes a release. It reads the publishable crates and their order from
`cargo metadata` and asks crates.io's sparse index which names and versions exist. On release day,
once the release pull request is merged:

```sh
git tag -a v<version> -m v<version> <release commit>
git worktree add --detach ../ulo-release v<version>    # a checkout with no untracked files
cd ../ulo-release
python3 scripts/publish.py plan                 # reads only: order, new and existing names, wall time
python3 scripts/publish.py publish --dry-run    # packages and verifies every crate still to publish
python3 scripts/publish.py publish              # uploads, and tags each leaf
git push origin $(git tag --points-at HEAD)     # the core tag and the leaf tags, nothing else
```

The script refuses a tree with any change or untracked file, which `git status --porcelain` lists,
so it runs in a checkout made for the release rather than one holding other work.

**Plan.** Prints the crates still to publish in upload order, marks each name new or existing, and
the wall time the limits below impose. It publishes and tags nothing.

**Publish.** Requires a clean tree and a HEAD that the core tag `v<version>` names, so the release
commit is tagged first. A crate is uploaded once every publishable workspace crate it depends on,
through a normal or build dependency, is on crates.io, and a name crates.io already has goes before
a new name that is ready at the same time. Cargo strips a dev-dependency that names no version from
the published manifest, so it imposes no order; `ulo-rpc-conformance` and `ulo-rpc-tcp` are each a
dev-dependency of the other, and so are `ulo-runtime-conformance` and `ulo-runtime-smol`.
`ulo-http-conformance` depends on `ulo-http-hyper` through a normal dependency, so the reference
host goes first; its dev-dependency on the suite, and the suite's on itself, impose nothing. A
dev-dependency that names a version stays in the manifest and is resolved on crates.io, so it orders
like a normal one: `ulo-grpc` names `ulo-runtime-tokio`'s. `--dry-run` runs one
`cargo publish --workspace --dry-run`, excluding the crates already on crates.io at their version:
Cargo packages the rest together and resolves their `=` pins on each other, so every crate still to
publish is built and verified. It needs the clean tree and not the tag.

**The new-name limit.** crates.io lets an account create five new names at once and then one every
ten minutes; a new version of an existing name has a burst of thirty and then one per minute
(https://crates.io/docs/rate-limits). A release with many new names therefore takes hours by
design, and `plan` prints the figure. When
crates.io answers 429 the script sleeps until the time the answer names plus 30 seconds and retries
the same crate.

**Rerun.** The registry is the state. After any interruption, a network failure or Ctrl-C, running
the same command resumes: a crate on crates.io at its version, or one Cargo reports as already
uploaded, is skipped. Any other failure stops the run with Cargo's output.

**Tags.** The core tag `v<version>` is created by hand on the release commit before publishing; the
script checks it. Each crate outside the `core` group of `release-plz.toml` gets an annotated
`<crate>-v<version>` tag at HEAD once it is on crates.io, created by the script and local. A crate
an earlier run uploaded is tagged only when its published `.crate` records HEAD as the commit it was
packaged from; the run names the commit it records otherwise. Pushing the tags is the last step.

`cargo publish --workspace` also uploads in dependency order, but stops at the first 429 and has no
resume. A crate checked alone, `cargo package -p <crate>`, resolves its `=` pins on crates.io
instead of in the workspace.

crates.io makes the account that publishes a new crate its only owner. Once a new crate is
published, the publishers team is added as an owner:

```sh
cargo owner --add github:ulo-rs:publishers <crate>
```
