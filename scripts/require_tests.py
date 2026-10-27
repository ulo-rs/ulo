"""Runs a command and fails unless every named test ran and passed.

A feature-gated test that a cfg typo compiles away reports `ok. 0 passed` and exits 0; matching
the names against libtest's `test <name> ... ok` lines turns that into a failure. A name matches
a test whose path is the name or ends in `::<name>`.

usage: require_tests.py <test name>... -- <command> [args...]

Standard library only, and Python 3.9.
"""

import re
import subprocess
import sys

from ran_a_test import RAN


def passed(lines):
    """The names of the tests in `lines` that ran and passed."""
    names = []
    for line in lines:
        m = RAN.match(line)
        if m and m.group(2) == "ok":
            names.append(m.group(1))
    return names


def missing(wanted, lines):
    """The names in `wanted` that no passing test in `lines` answers to."""
    ok = passed(lines)
    return [w for w in wanted if not any(n == w or n.endswith("::" + w) for n in ok)]


def main(argv):
    if "--" not in argv:
        print(__doc__, file=sys.stderr)
        return 2
    cut = argv.index("--")
    wanted, command = argv[:cut], argv[cut + 1 :]
    if not wanted or not command:
        print(__doc__, file=sys.stderr)
        return 2
    proc = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
    lines = []
    for line in proc.stdout:
        sys.stdout.write(line)
        lines.append(line.rstrip("\n"))
    status = proc.wait()
    absent = missing(wanted, lines)
    for name in absent:
        print(f"require_tests.py: {name} did not run and pass", file=sys.stderr)
    return status or (1 if absent else 0)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
