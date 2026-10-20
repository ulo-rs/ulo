#!/usr/bin/env python3
"""One version of `sqlx` and `sqlx-core` in the workspace's own tree.

    python3 scripts/one_sqlx.py            # reads the tree, fails on two versions of either
    python3 scripts/one_sqlx.py --locked   # the same, cargo refusing to touch Cargo.lock
    python3 scripts/one_sqlx.py --self-test

The SQL stores share one SQLx: `ulo-db-sqlx` hands its pool to SeaORM's connector, and a second
SQLx version in the tree would make that pool a different type from the one SeaORM names, or make
two copies of the driver fight over the same database. The check reads

    cargo tree --workspace --edges normal --all-features --target=all --prefix none

(normal edges only: a dev-dependency on another version is not shipped, and every feature on, since
the stores pick their drivers by feature) and fails when `sqlx` or `sqlx-core` appears at more than
one version.

`--self-test` runs the reader over a sample that holds two versions of each and fails unless it is
refused naming both crates, then over a sample that holds one of each and fails unless it passes.
The real check runs the self-test first, so a reader that stopped recognising the tree's format
cannot report a clean tree.

Exit status: 0 when the tree holds one version of each, 1 when it holds two, 2 on a cargo error.
Standard library only, and Python 3.9, so it runs on a stock runner and a stock macOS.
"""

import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
WATCHED = ("sqlx", "sqlx-core")

# `sqlx v0.8.6`, `sqlx-core v0.8.6 (*)`, `ulo-db-sqlx v0.1.0 (/path)`: the first token is the name,
# after whatever drawing characters a prefix other than `none` would print.
NODE = re.compile(r"^[\s|`\\+-]*(?P<name>[A-Za-z0-9_-]+) v(?P<version>\S+)")


def versions(tree):
    """The versions of each watched crate that `tree` (cargo tree's text) prints."""
    found = {name: set() for name in WATCHED}
    for line in tree.splitlines():
        m = NODE.match(line)
        if m and m["name"] in found:
            found[m["name"]].add(m["version"])
    return found


def findings(tree):
    return [
        f"{name} appears at {len(vs)} versions: {', '.join(sorted(vs))}"
        for name, vs in versions(tree).items()
        if len(vs) > 1
    ]


VIOLATING = """\
ulo-db-sqlx v0.1.0 (/w/crates/ulo-db-sqlx)
|-- sqlx v0.8.6
|   `-- sqlx-core v0.8.6
`-- sea-orm v1.1.0
    |-- sqlx v0.7.4
    |   `-- sqlx-core v0.7.4
    `-- sqlx-core v0.8.6 (*)
"""

VALID = """\
ulo-db-sqlx v0.1.0 (/w/crates/ulo-db-sqlx)
|-- sqlx v0.8.6
|   `-- sqlx-core v0.8.6
`-- sea-orm v1.1.0
    |-- sqlx v0.8.6 (*)
    `-- sqlx-core v0.8.6 (*)
ulo-db-redis v0.1.0 (/w/crates/ulo-db-redis)
"""


def self_test():
    bad = findings(VIOLATING)
    if len(bad) != 2 or not any(f.startswith("sqlx appears") for f in bad) or not any(
        f.startswith("sqlx-core appears") for f in bad
    ):
        print(f"self-test: the violating sample was not refused for both crates: {bad}", file=sys.stderr)
        return False
    if findings(VALID):
        print(f"self-test: the valid sample was refused: {findings(VALID)}", file=sys.stderr)
        return False
    if not all(versions(VALID)[n] for n in WATCHED):
        print("self-test: the valid sample's versions were not read", file=sys.stderr)
        return False
    return True


def main(argv):
    if "--self-test" in argv:
        return 0 if self_test() else 1
    if not self_test():
        return 1
    cmd = ["cargo", "tree", "--workspace", "--edges", "normal", "--all-features", "--target=all", "--prefix", "none"]
    if "--locked" in argv:
        cmd.append("--locked")
    run = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True)
    if run.returncode != 0:
        print(f"{' '.join(cmd)} failed:\n{run.stderr}", file=sys.stderr)
        return 2
    seen = versions(run.stdout)
    bad = findings(run.stdout)
    if not any(seen.values()):
        print("no sqlx or sqlx-core in the workspace tree: the reader found nothing to check", file=sys.stderr)
        return 2
    for f in bad:
        print(f, file=sys.stderr)
    if bad:
        return 1
    print("; ".join(f"{n} {', '.join(sorted(v)) or 'absent'}" for n, v in seen.items()))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
