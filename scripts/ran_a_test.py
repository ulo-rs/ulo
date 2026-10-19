"""The one definition of "ran a test", shared by `unsafe_allowed.py miri` and `template_matrix.py`.

libtest prints `test <name> ... ok` or `... FAILED` for a test it ran and `... ignored` for one it
did not, so a run that executed nothing prints no such line. A caller that must see at least one
test run counts these lines rather than trusting an exit status of 0, which `0 passed` also gives.

Standard library only, and Python 3.9.
"""

import re

# libtest's line for a test it ran: `test <name> ... ok`, `FAILED`, or `ignored` for one it did not.
RAN = re.compile(r"^test (\S+)(?: - should panic)? \.\.\. (ok|FAILED)\s*$")


def ran_a_test(line, prefix=None):
    """True when `line` reports a test that ran, and, given `prefix`, one whose name starts with it."""
    m = RAN.match(line)
    return bool(m) and (prefix is None or m.group(1).startswith(prefix))


def count_ran(lines, prefix=None):
    """How many of `lines` report a test that ran."""
    return sum(1 for line in lines if ran_a_test(line, prefix))
