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
   break must be accepted in `semver-breaks.txt`. Any other version is a major release, which
   accepts its breaks without an entry; it is still run as a compatible release, which lists them
   in the log, since a major release type skips every lint. Every crate must report more than zero
   checks run, so a run that skipped its lints fails rather than passing empty. Proc-macro crates
   and `ulo-cli`, a binary, have no library API to compare and are named apart in the job's last
   line.

## Reading a failure

Each finding starts a line on stderr, prefixed with its step; a finding about a break lists the
items it counts on the lines below it. A message with no prefix and exit status 2 is a usage
error or a malformed line in `semver-breaks.txt`. One prefixed `self-test:` means the script no
longer refuses a violation it plants, a fault in the script rather than in the tree.

| Finding | What to do |
| --- | --- |
| `lockstep: X ..., so it belongs in group core` | Add a `[[package]]` for `X` with `version_group = "core"` and `git_tag_enable = false`. |
| `lockstep: X is in group core but ...` | Remove its `[[package]]`: nothing makes it part of the core. |
| `lockstep: X is at V, the core at W` or `X is at V, and every crate is at the core's W ...` | Give `X` the workspace version again. |
| `release-plz: update exited N` | The log above it carries release-plz's error, such as a key `release-plz.toml` may not hold. |
| `release-plz: release-plz did not version X` or `determined no version` | release-plz passed over `X`, or over every crate: its log above shows what it printed instead. |
| `release-plz: after the update, ...` | The versions release-plz would write break the lockstep rule; the rest of the line is a `lockstep:` finding. |
| `semver: X breaks L and semver-breaks.txt does not accept it` | Restore the item, or, when the break is wanted, add the `break` line the finding prints, with its reason. |
| `semver: X breaks L in N items and semver-breaks.txt accepts M` | The finding lists the items now reported; set the count to their number. |
| `semver: semver-breaks.txt accepts X L, which this run did not report` | Remove the entry. |
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
regenerates `Cargo.lock` for the workspace members, and updates the Install sections of `README.md`
and `crates/ulo/README.md`, which `scripts/check_readme_install.py` holds to the workspace version.
While that version is a pre-release the Install sections pin each crate `=<version>`, and the
script refuses any other operator: a caret on a pre-release also takes a later pre-release of the
same version, which may break the first.
The release is tagged `v<version>`. From that tag on, `release check` compares with the new release,
every entry in `semver-breaks.txt` stops matching, and the first change after the tag removes them.
Until that change merges, the job fails on `master` and on every other pull request.
