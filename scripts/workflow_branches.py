#!/usr/bin/env python3
"""Workflow triggers must run for the repository's default branch.

    python3 scripts/workflow_branches.py check [BRANCH]
    python3 scripts/workflow_branches.py --self-test

A `push`, `pull_request` or `pull_request_target` trigger that filters on `branches` runs only
for the branches its patterns match, and one that filters on `branches-ignore` for every branch
its patterns do not. After a default-branch rename, or the deletion of the branch a workflow was
written for, such a workflow stops running and nothing reports it. `check` fails when any
`.github/workflows/*.yml` has such a trigger whose filter leaves the default branch out, read
with GitHub's pattern rules for `*`, `**` and a leading `!`, the last matching pattern deciding.
A trigger with neither key runs for every branch and passes. A trigger written in a shape this
reader does not know is refused rather than passed.

BRANCH is the default branch; without it, the last component of
`git symbolic-ref refs/remotes/origin/HEAD`, and `master` when that is unset. On a runner
`actions/checkout` sets no `origin/HEAD`, so the workflow passes
`${{ github.event.repository.default_branch }}`. `--self-test` runs the rule over planted
workflows that leave the default out and fails unless each is refused, and over ones that keep
it and fails unless each passes.

Standard library only, and Python 3.9. The `on:` block is read by indentation, since PyYAML is
not on a stock runner: block and flow lists, a scalar, and a one-line flow mapping.
"""
import glob
import re
import subprocess
import sys

TRIGGERS = ("push", "pull_request", "pull_request_target")
FILTERS = ("branches", "branches-ignore")


def default_branch():
    try:
        out = subprocess.run(
            ["git", "symbolic-ref", "refs/remotes/origin/HEAD"],
            capture_output=True, text=True, check=True,
        ).stdout.strip()
    except (OSError, subprocess.CalledProcessError):
        return "master"
    return out.rsplit("/", 1)[-1] or "master"


def indent_of(line):
    return len(line) - len(line.lstrip(" "))


def unquote(item):
    return item.strip().strip("'\"")


def flow_items(text):
    return [unquote(x) for x in text.strip().strip("[]").split(",") if x.strip()]


def pattern_matches(pattern, branch):
    regex = ""
    i = 0
    while i < len(pattern):
        if pattern.startswith("**", i):
            regex += ".*"
            i += 2
        elif pattern[i] == "*":
            regex += "[^/]*"
            i += 1
        else:
            regex += re.escape(pattern[i])
            i += 1
    return re.fullmatch(regex, branch) is not None


def selected(patterns, branch):
    """Whether the patterns select `branch`: the last pattern matching it decides."""
    chosen = False
    for pattern in patterns:
        negated = pattern.startswith("!")
        if pattern_matches(pattern[1:] if negated else pattern, branch):
            chosen = not negated
    return chosen


def filters_of(body, header):
    """`{key: patterns}` for the filter keys of one trigger, from its body lines or its one-line
    flow mapping `header`; a string naming a shape that could not be read."""
    found = {}
    if header.startswith("{"):
        for key in FILTERS:
            m = re.search(r"(?:^|[{,\s])" + re.escape(key) + r"""["']?\s*:\s*(\[[^\]]*\]|[^,}]+)""", header)
            if m:
                found[key] = flow_items(m.group(1))
        return found
    if header not in ("", "null", "~", "{}"):
        return f"a trigger value `{header}` that is not a mapping"
    for k, l in enumerate(body):
        m = re.match(r"""^(\s+)["']?(branches|branches-ignore)["']?\s*:\s*(.*)$""", l)
        if not m:
            continue
        key, rest = m.group(2), m.group(3).strip()
        if rest.startswith("["):
            found[key] = flow_items(rest)
        elif rest:
            found[key] = [unquote(rest)]
        else:
            names = []
            for item in body[k + 1:]:
                stripped = item.strip()
                if indent_of(item) < len(m.group(1)) or not stripped.startswith("-"):
                    if indent_of(item) <= len(m.group(1)):
                        break
                    return f"`{key}` holding `{stripped}`"
                names.append(unquote(stripped[1:]))
            found[key] = names
    return found


def findings(text, branch):
    """The triggers in `text` that do not run for `branch`."""
    lines = [l.split(" #")[0].rstrip() for l in text.splitlines()]
    lines = [l for l in lines if l.strip() and not l.lstrip().startswith("#")]
    start = next((i for i, l in enumerate(lines) if re.match(r"""^["']?on["']?\s*:""", l)), None)
    if start is None:
        return []
    block = []
    for l in lines[start + 1:]:
        if indent_of(l) == 0:
            break
        block.append(l)
    found = []
    i = 0
    while i < len(block):
        m = re.match(r"""^(\s+)["']?(push|pull_request|pull_request_target)["']?\s*:\s*(.*)$""", block[i])
        if not m:
            i += 1
            continue
        trigger, base, header = m.group(2), len(m.group(1)), m.group(3).strip()
        j = i + 1
        while j < len(block) and indent_of(block[j]) > base:
            j += 1
        filters = filters_of(block[i + 1:j], header)
        if isinstance(filters, str):
            found.append(f"{trigger} has {filters}, which this check cannot read")
        elif "branches" in filters and not selected(filters["branches"], branch):
            found.append(f"{trigger} lists branches {filters['branches']}, which leave out `{branch}`")
        elif "branches-ignore" in filters and selected(filters["branches-ignore"], branch):
            found.append(f"{trigger} ignores branches {filters['branches-ignore']}, which take in `{branch}`")
        i = j
    return found


def check(branch):
    paths = sorted(glob.glob(".github/workflows/*.yml") + glob.glob(".github/workflows/*.yaml"))
    if not paths:
        print("no workflows under .github/workflows", file=sys.stderr)
        return 1
    bad = 0
    for path in paths:
        with open(path) as f:
            for finding in findings(f.read(), branch):
                print(f"{path}: {finding}", file=sys.stderr)
                bad += 1
    if bad:
        print(f"{bad} trigger(s) would not run for `{branch}`", file=sys.stderr)
        return 1
    print(f"{len(paths)} workflow(s) trigger on `{branch}`")
    return 0


PLANTED = {
    "flow list": "name: X\non:\n  push:\n    branches: [experiment/old]\n  pull_request:\n    branches: [master]\n",
    "block list": "name: X\non:\n  pull_request:\n    branches:\n      - 'experiment/old'\n      - release\n",
    "branches-ignore": "on:\n  push:\n    branches-ignore: [master]\n",
    "negated": "on:\n  push:\n    branches: ['**', '!master']\n",
    "flow mapping": "on:\n  push: {branches: [old]}\n",
    "quoted key": "on:\n  \"pull_request\":\n    branches: [old]\n",
    "pull_request_target": "on:\n  pull_request_target:\n    branches: [old]\n",
    "unreadable": "on:\n  push:\n    branches:\n      name: master\n",
}
VALID = {
    "flow list": "name: X\non:\n  push:\n    branches: [master]\n  pull_request:\n    branches: [master, dev]\n",
    "block list": "on:\n  push:\n    branches:\n      - master\njobs:\n  a:\n    steps: []\n",
    "same-indent block list": "on:\n  push:\n    branches:\n    - \"master\"\n",
    "scalar": "on:\n  push:\n    branches: master\n",
    "glob": "on:\n  push:\n    branches: ['**']\n  pull_request:\n    branches: ['mas*']\n",
    "ignore another": "on:\n  push:\n    branches-ignore: [old, 'release/**']\n",
    "no filter": "on:\n  push:\n  pull_request:\n",
    "unfiltered flow": "on: [push]\n",
}


def self_test():
    ok = True
    for name, text in PLANTED.items():
        if not findings(text, "master"):
            print(f"self-test: planted `{name}` workflow was not refused", file=sys.stderr)
            ok = False
    for name, text in VALID.items():
        got = findings(text, "master")
        if got:
            print(f"self-test: valid `{name}` workflow was refused: {got}", file=sys.stderr)
            ok = False
    return 0 if ok else 1


def main(argv):
    if argv[1:2] == ["--self-test"]:
        return self_test()
    if argv[1:2] == ["check"]:
        if self_test():
            return 1
        branch = argv[2] if len(argv) > 2 and argv[2] else default_branch()
        return check(branch)
    print(__doc__, file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv))
