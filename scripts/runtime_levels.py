#!/usr/bin/env python3
"""Each workspace member held to the runtime level `runtime-levels.txt` gives it.

    python3 scripts/runtime_levels.py            # reads the trees, fails on a crate off its level
    python3 scripts/runtime_levels.py --locked   # the same, cargo refusing to touch Cargo.lock

The two levels are the transports DESIGN's §8, "Two levels of runtime independence".
*Runtime-free*: no `tokio` crate anywhere in the crate's normal dependency tree, the level that
lets anyone implement a plug point without tokio. *Runtime-neutral*: tokio only with features that
contain no runtime, `default` (empty) and `sync`, which hyper brings, and on the HTTP/2 path also
`io-util` and `bytes`, which h2 brings. A *tokio-bound* crate is listed so that no member goes
unclassified, and is not checked.

The path rule: in a runtime-neutral tree, tokio arrives only through hyper, h2 or tokio-util, so
every package that depends on tokio directly is one of those three, and a workspace member that
depends on tokio-util directly has a `direct-tokio-util` line in the list giving the reason.

For every checked entry the script runs

    cargo tree -p <crate> <flags> -e normal,features -i tokio --target=all --prefix depth

and reads the `tokio feature "<name>"` nodes, each a feature enabled in that tree. `--target=all`
reads the dependencies of every platform, so the verdict is the same on any host and a tokio that
only one platform's dependencies bring is not missed. cargo answers a tree without tokio in one of
two ways, and both pass: "did not match any packages" (exit 101) when tokio is not in the resolved
graph, "nothing to print" (exit 0) when it is there by a dev- or build-dependency edge alone. Any
other non-zero exit is a cargo error, reported as an error rather than a verdict.

When tokio is in the tree, the script reads it a second time with `-e normal`, which prints package
nodes alone, and takes each line's parent as the package it depends on: the children of `tokio` are
its direct dependents, printed as `through`, and the children of `tokio-util` are tokio-util's.
cargo prints a repeated node's children only at its first occurrence, and every edge as a line, so
no direct dependent is hidden by the `(*)` de-duplication.

It fails when a runtime-free entry's tree contains tokio; when a runtime-neutral entry's tree
enables a tokio feature outside its level's set, has a package other than hyper, h2 or tokio-util
depending on tokio directly, or has a workspace member depending on tokio-util directly that no
`direct-tokio-util` line names; when a `direct-tokio-util` line names a member no runtime-neutral
tree has depending on tokio-util directly; and when a workspace member has no entry. A list line
naming no member is an error before any tree is read: given `-i`, cargo widens an unknown `-p` to
the whole workspace rather than refusing it.

Exit status: 0 when every entry holds, 1 when one does not or a member is unclassified, 2 on a
cargo error or a malformed list. Standard library only, and Python 3.9, so it runs on a stock
runner and a stock macOS.
"""

import json
import re
import shlex
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
LIST = ROOT / "runtime-levels.txt"

# The tokio features each checked level allows. `default` is tokio's empty default feature.
ALLOWED = {
    "runtime-free": None,
    "runtime-neutral": {"default", "sync"},
    "runtime-neutral+http2": {"default", "sync", "io-util", "bytes"},
}
LEVELS = (*ALLOWED, "tokio-bound")

# The packages a runtime-neutral tree may take tokio through: hyper names it for `sync`, h2 for
# `io-util`, and tokio-util is h2's.
CARRIERS = ("hyper", "h2", "tokio-util")
DIRECT_TOKIO_UTIL = "direct-tokio-util"

FEATURE = re.compile(r'^tokio feature "([^"]+)"')
PACKAGE = re.compile(r"^(\S+) v\d")
DEPTH = re.compile(r"^(\d+)(.*)$")
NOT_IN_GRAPH = "package ID specification `tokio` did not match any packages"
NOTHING_TO_PRINT = "nothing to print"


class ListError(Exception):
    pass


def valid_flags(flags):
    """Only feature selection: the list chooses a crate's features, never what the tree reads."""
    i = 0
    while i < len(flags):
        flag = flags[i]
        if flag in ("--all-features", "--no-default-features") or flag.startswith("--features="):
            i += 1
        elif flag in ("--features", "-F") and i + 1 < len(flags):
            i += 2
        else:
            return False
    return True


def entries():
    """`(levels, allowances)`: the level entries, and the `direct-tokio-util` lines by crate."""
    found, allowances = [], {}
    seen = set()
    for number, raw in enumerate(LIST.read_text().splitlines(), 1):
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        # Split before shlex: a reason is prose, and an apostrophe in it is no quote.
        head = line.split(None, 2)
        if len(head) >= 2 and head[1] == DIRECT_TOKIO_UTIL:
            crate, reason = head[0], head[2].strip() if len(head) == 3 else ""
            if not reason:
                raise ListError(f"runtime-levels.txt:{number}: a `{DIRECT_TOKIO_UTIL}` line gives its reason after the keyword, got `{line}`")
            if crate in allowances:
                raise ListError(f"runtime-levels.txt:{number}: `{crate}` has a second `{DIRECT_TOKIO_UTIL}` line")
            allowances[crate] = number
            continue
        tokens = shlex.split(line)
        if len(tokens) < 2:
            raise ListError(f"runtime-levels.txt:{number}: expected `<crate> <level> [flags]`, got `{line}`")
        crate, level, flags = tokens[0], tokens[1], tokens[2:]
        if level not in LEVELS:
            raise ListError(f"runtime-levels.txt:{number}: `{level}` is no level; one of {', '.join(LEVELS)}")
        if not valid_flags(flags):
            raise ListError(f"runtime-levels.txt:{number}: flags may only select features (--features, -F, --all-features, --no-default-features), got `{' '.join(flags)}`")
        key = (crate, tuple(flags))
        if key in seen:
            raise ListError(f"runtime-levels.txt:{number}: `{crate}` with `{' '.join(flags)}` is listed twice")
        seen.add(key)
        found.append((number, crate, level, flags))
    return found, allowances


def members(locked):
    command = ["cargo", "metadata", "--no-deps", "--format-version", "1", *locked]
    out = subprocess.run(command, cwd=ROOT, capture_output=True, text=True)
    if out.returncode != 0:
        raise ListError(f"`{' '.join(command)}` failed:\n{out.stderr.strip()}")
    meta = json.loads(out.stdout)
    ids = set(meta["workspace_members"])
    return sorted(p["name"] for p in meta["packages"] if p["id"] in ids)


def tree_lines(command, out):
    """`(depth, node)` for each line of a `--prefix depth` tree, `(*)` dropped."""
    lines = []
    for line in out.stdout.splitlines():
        m = DEPTH.match(line)
        if not m:
            if line.strip():
                raise RuntimeError(f"`{' '.join(command)}` printed a line with no depth: {line!r}")
            continue
        lines.append((int(m.group(1)), m.group(2).replace(" (*)", "")))
    if not lines or lines[0][0] != 0 or not PACKAGE.match(lines[0][1]) or PACKAGE.match(lines[0][1]).group(1) != "tokio":
        raise RuntimeError(f"`{' '.join(command)}` printed a tree whose root is not tokio:\n{out.stdout[:400]}")
    return lines


def read_features(crate, flags, locked):
    """`(features, None)` when tokio is in the tree, `(None, why)` when it is not.

    Raises `RuntimeError` carrying cargo's output on any other answer.
    """
    command = ["cargo", "tree", "-p", crate, *flags, "-e", "normal,features", "-i", "tokio",
               "--target=all", "--prefix", "depth", *locked]
    out = subprocess.run(command, cwd=ROOT, capture_output=True, text=True)
    if out.returncode != 0:
        if NOT_IN_GRAPH in out.stderr and not out.stdout.strip():
            return None, "did not match any packages"
        raise RuntimeError(f"`{' '.join(command)}` exited {out.returncode}:\n{out.stderr.strip()}")
    if not out.stdout.strip():
        if NOTHING_TO_PRINT in out.stderr:
            return None, "nothing to print"
        raise RuntimeError(f"`{' '.join(command)}` printed nothing and did not say why:\n{out.stderr.strip()}")
    features = set()
    for _, node in tree_lines(command, out)[1:]:
        feature = FEATURE.match(node)
        if feature:
            features.add(feature.group(1))
    return features, None


def read_dependents(crate, flags, locked):
    """Each package in the tree mapped to the packages that depend on it directly.

    Read from the inverted tree of package nodes alone, where a line's parent is the package it
    depends on. Raises `RuntimeError` carrying cargo's output when the tree cannot be read.
    """
    command = ["cargo", "tree", "-p", crate, *flags, "-e", "normal", "-i", "tokio",
               "--target=all", "--prefix", "depth", *locked]
    out = subprocess.run(command, cwd=ROOT, capture_output=True, text=True)
    if out.returncode != 0 or not out.stdout.strip():
        raise RuntimeError(f"`{' '.join(command)}` exited {out.returncode} with no tree to read:\n{out.stderr.strip()}")
    dependents, path = {}, []
    for depth, node in tree_lines(command, out):
        package = PACKAGE.match(node)
        if not package or depth > len(path):
            raise RuntimeError(f"`{' '.join(command)}` printed a line that is no package under its parent: {depth}{node}")
        del path[depth:]
        if path:
            dependents.setdefault(path[-1], set()).add(package.group(1))
        path.append(package.group(1))
    return dependents


def main(argv):
    locked = ["--locked"] if "--locked" in argv else []
    unknown = [a for a in argv if a != "--locked"]
    if unknown:
        print(f"usage: runtime_levels.py [--locked]; unknown argument {unknown[0]}", file=sys.stderr)
        return 2

    try:
        listed, allowances = entries()
        workspace = members(locked)
    except ListError as e:
        print(f"error: {e}")
        return 2

    failures, errors = [], []
    for number, crate, _, _ in listed:
        if crate not in workspace:
            errors.append(f"runtime-levels.txt:{number}: `{crate}` is no workspace member")
    for crate, number in allowances.items():
        if crate not in workspace:
            errors.append(f"runtime-levels.txt:{number}: `{DIRECT_TOKIO_UTIL}` names `{crate}`, which is no workspace member")
    classified = {crate for _, crate, _, _ in listed}
    for crate in workspace:
        if crate not in classified:
            failures.append(f"{crate}: a workspace member runtime-levels.txt does not classify")
    if errors:
        for line in failures:
            print(f"fail: {line}")
        for line in errors:
            print(f"error: {line}")
        return 2

    carriers = ", ".join(CARRIERS[:-1]) + f" or {CARRIERS[-1]}"
    used = set()
    width = max(len(crate) + 1 + len(" ".join(flags)) for _, crate, _, flags in listed)
    for _, crate, level, flags in listed:
        name = f"{crate} {' '.join(flags)}".strip()
        if level == "tokio-bound":
            print(f"{'skip':<5} {level:<22} {name:<{width}}  not checked")
            continue
        try:
            features, why = read_features(crate, flags, locked)
            dependents = None if features is None else read_dependents(crate, flags, locked)
        except RuntimeError as e:
            errors.append(str(e))
            print(f"{'ERROR':<5} {level:<22} {name:<{width}}  cargo error, below")
            continue
        if features is None:
            print(f"{'ok':<5} {level:<22} {name:<{width}}  no tokio ({why})")
            continue
        direct = sorted(dependents.get("tokio", ()))
        shown = f"tokio [{', '.join(sorted(features))}] through {', '.join(direct)}"
        allowed = ALLOWED[level]
        if allowed is None:
            failures.append(f"{name}: runtime-free, but its tree contains {shown}")
            print(f"{'FAIL':<5} {level:<22} {name:<{width}}  {shown}")
            continue
        found = []
        outside = sorted(features - allowed)
        if outside:
            found.append(f"{name}: {level} allows tokio's {', '.join(sorted(allowed))}, but its tree enables {', '.join(outside)}")
        for package in direct:
            if package not in CARRIERS:
                who = f"workspace member {package}" if package in workspace else f"{package}, no workspace member,"
                found.append(f"{name}: {level} takes tokio only through {carriers}, but {who} depends on tokio directly")
        listed_util = []
        for package in sorted(dependents.get("tokio-util", ())):
            if package not in workspace:
                continue
            if package in allowances:
                used.add(package)
                listed_util.append(package)
            else:
                found.append(f"{name}: {level}, but workspace member {package} depends on tokio-util directly and runtime-levels.txt has no `{DIRECT_TOKIO_UTIL}` line for it")
        if listed_util:
            shown += f"; tokio-util direct from {', '.join(listed_util)}, listed"
        failures.extend(found)
        print(f"{'FAIL' if found else 'ok':<5} {level:<22} {name:<{width}}  {shown}")

    # A cargo error leaves a tree unread, and with it any use of an allowance.
    if not errors:
        for crate, number in sorted(allowances.items(), key=lambda item: item[1]):
            if crate not in used:
                failures.append(f"runtime-levels.txt:{number}: `{DIRECT_TOKIO_UTIL}` for {crate}, but no runtime-neutral entry's tree has {crate} depending on tokio-util directly")

    for line in failures:
        print(f"fail: {line}")
    for line in errors:
        print(f"error: {line}")
    checked = sum(1 for e in listed if e[2] != "tokio-bound")
    print(f"{len(workspace)} members, {len(listed)} entries, {checked} checked, {len(failures)} failures, {len(errors)} errors")
    if errors:
        return 2
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
