#!/usr/bin/env python3
"""Holds the `compile_fail` cases of `crates/ulo-codegen-tests/tests/ui/` to their error codes on a
compiler other than the one their snapshots were written by.

    python3 scripts/ui_codes.py +1.88            # run the cases on 1.88 and compare codes
    python3 scripts/ui_codes.py --self-test      # the verdicts, run against planted outputs

The `.stderr` snapshots are rustc 1.98.1's wording, checked byte for byte by the `macro
diagnostics` job on that compiler alone. On the workspace's floor the wording differs, so this
check compares what does not: the set of `error[E....]` codes each case prints. A case fails when
it compiles, when it stops failing, or when a code appears or disappears, unless
`ci/ui-codes-1.88.txt` names the case and the exact set the floor prints instead. An allowance the
run does not need fails too, so the list stays the set of real differences.

The cases run through trybuild, `cargo test -p ulo-codegen-tests --test compile_fail -- --ignored`:
the test is ignored on any compiler but the snapshots', and `--ignored` runs it there, every
mismatch reported with the output rustc gave. `TRYBUILD` is removed from the environment, so no
snapshot is rewritten, and `TERM=dumb` keeps the output free of a diff's markup.

Standard library only, and Python 3.9.
"""

import os
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CRATE = ROOT / "crates" / "ulo-codegen-tests"
UI = CRATE / "tests" / "ui"
ALLOWANCES = ROOT / "ci" / "ui-codes-1.88.txt"

CODE = re.compile(r"error\[(E\d{4})\]")
ANSI = re.compile(r"\x1b\[[0-9;]*m")
# trybuild's line for one case: `test tests/ui/name.rs ... ok`, `... mismatch` or `... error`.
CASE = re.compile(r"^test (tests/ui/[^ ]+\.rs) \.\.\. (\w+)\s*$")


def codes(text: str) -> frozenset:
    return frozenset(CODE.findall(text))


def parse(output: str) -> dict:
    """Each case trybuild reported, as `(status, actual codes or None)`: the codes of the output
    rustc gave for a mismatch, `None` for an `ok`, whose output is its snapshot's."""
    lines = ANSI.sub("", output).splitlines()
    cases = {}
    current = None
    block = []
    for line in lines + ["test tests/ui/<end>.rs ... end"]:
        found = CASE.match(line)
        if found:
            if current is not None:
                cases[current[0]] = settle(current[1], block)
            current, block = (Path(found.group(1)).stem, found.group(2)), []
        elif current is not None:
            block.append(line)
    cases.pop("<end>", None)
    return cases


def settle(status: str, block: list):
    text = "\n".join(block)
    if status == "ok":
        return ("ok", None)
    if status == "mismatch":
        actual = text.split("ACTUAL OUTPUT:", 1)[1] if "ACTUAL OUTPUT:" in text else ""
        actual = actual.split("note: If the actual output", 1)[0]
        return ("mismatch", codes(actual))
    if "Expected test case to fail to compile, but it succeeded" in text:
        return ("compiled", None)
    return (status, None)


def read_allowances(text: str) -> dict:
    """`<case> <codes, space separated or `-` for none>  # reason` per line."""
    allowed = {}
    for line in text.splitlines():
        line = line.split("#", 1)[0].strip()
        if not line:
            continue
        name, *rest = line.split()
        allowed[name] = frozenset(code for code in rest if code != "-")
    return allowed


def verdicts(cases: dict, snapshots: dict, allowed: dict) -> list:
    """Why each case or allowance fails; empty when every case holds."""
    failures = []
    for name in sorted(snapshots):
        if name not in cases:
            failures.append(f"{name}: trybuild reported no result for it")
            continue
        status, actual = cases[name]
        expected = snapshots[name]
        if status == "compiled":
            failures.append(f"{name}: compiled, where it must fail")
            continue
        if status not in ("ok", "mismatch"):
            failures.append(f"{name}: trybuild answered `{status}`")
            continue
        got = expected if actual is None else actual
        if name in allowed:
            if got == expected:
                failures.append(f"{name}: allowed {sorted(allowed[name])}, and prints the snapshot's {sorted(expected)}; remove the allowance")
            elif got != allowed[name]:
                failures.append(f"{name}: prints {sorted(got)}, where the allowance names {sorted(allowed[name])} and the snapshot {sorted(expected)}")
        elif got != expected:
            failures.append(f"{name}: prints {sorted(got)}, where the snapshot has {sorted(expected)}")
    for name in sorted(set(allowed) - set(snapshots)):
        failures.append(f"{name}: allowed, and no such case exists")
    return failures


def snapshots_of(directory: Path) -> dict:
    return {path.stem: codes(path.read_text()) for path in sorted(directory.glob("*.stderr"))}


def run(toolchain: str) -> int:
    env = {key: value for key, value in os.environ.items() if key != "TRYBUILD"}
    env["TERM"] = "dumb"
    command = ["cargo"] + ([toolchain] if toolchain else [])
    command += ["test", "-p", "ulo-codegen-tests", "--test", "compile_fail", "--locked", "--", "--ignored", "--exact", "--nocapture", "mismatched_handlers_fail_to_compile"]
    done = subprocess.run(command, cwd=ROOT, env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
    snapshots = snapshots_of(UI)
    cases = parse(done.stdout)
    if not cases:
        print(done.stdout)
        print("trybuild reported no case: the run did not reach the suite", file=sys.stderr)
        return 1
    failures = verdicts(cases, snapshots, read_allowances(ALLOWANCES.read_text()))
    for line in failures:
        print(f"FAIL {line}")
    differing = sum(1 for status, _ in cases.values() if status == "mismatch")
    failed = {line.split(":", 1)[0] for line in failures}
    print(f"{len(set(snapshots) - failed)} of {len(snapshots)} cases held their codes; {differing} differ in wording")
    return 1 if failures else 0


PLANTED_OUTPUT = """
test tests/ui/same.rs ... ok
test tests/ui/reworded.rs ... mismatch

EXPECTED:
error[E0277]: the trait bound is not satisfied
ACTUAL OUTPUT:
error[E0277]: the trait bound was not satisfied
note: If the actual output is the correct output you can bless it by rerunning
test tests/ui/new_code.rs ... mismatch

EXPECTED:
error[E0277]: one
ACTUAL OUTPUT:
error[E0277]: one
error[E0599]: two
note: If the actual output is the correct output you can bless it by rerunning
test tests/ui/lost_code.rs ... mismatch

EXPECTED:
error[E0277]: one
error[E0308]: two
ACTUAL OUTPUT:
error[E0277]: one
note: If the actual output is the correct output you can bless it by rerunning
test tests/ui/allowed.rs ... mismatch

EXPECTED:
error[E0425]: one
ACTUAL OUTPUT:
error[E0412]: one
note: If the actual output is the correct output you can bless it by rerunning
test tests/ui/allowed_wrongly.rs ... mismatch

EXPECTED:
error[E0425]: one
ACTUAL OUTPUT:
error[E0433]: one
note: If the actual output is the correct output you can bless it by rerunning
test tests/ui/stale.rs ... ok
test tests/ui/compiles.rs ... error
Expected test case to fail to compile, but it succeeded.
"""

PLANTED_SNAPSHOTS = {
    "same": frozenset({"E0277"}),
    "reworded": frozenset({"E0277"}),
    "new_code": frozenset({"E0277"}),
    "lost_code": frozenset({"E0277", "E0308"}),
    "allowed": frozenset({"E0425"}),
    "allowed_wrongly": frozenset({"E0425"}),
    "stale": frozenset({"E0277"}),
    "compiles": frozenset({"E0277"}),
    "unreported": frozenset({"E0277"}),
}

PLANTED_ALLOWANCES = """
allowed E0412  # a planted allowance the run needs
allowed_wrongly E0412  # the run prints another code
stale E0412  # the run prints the snapshot's codes
gone E0277  # names no case
"""

# The cases each must fail, and those that must hold.
PLANTED_FAILING = {"new_code", "lost_code", "allowed_wrongly", "stale", "compiles", "unreported", "gone"}
PLANTED_HOLDING = {"same", "reworded", "allowed"}


def self_test() -> int:
    failures = verdicts(parse(PLANTED_OUTPUT), PLANTED_SNAPSHOTS, read_allowances(PLANTED_ALLOWANCES))
    failed = {line.split(":", 1)[0] for line in failures}
    bad = [f"{name}: expected a failure, got none" for name in sorted(PLANTED_FAILING - failed)]
    bad += [f"{name}: expected to hold, got: {line}" for name in sorted(PLANTED_HOLDING & failed) for line in failures if line.startswith(f"{name}:")]
    for line in bad:
        print(f"SELF-TEST FAIL {line}")
    if not bad:
        print(f"self-test: {len(PLANTED_FAILING)} planted violations refused and {len(PLANTED_HOLDING)} planted cases held")
    return 1 if bad else 0


def main(argv) -> int:
    if "--self-test" in argv:
        return self_test()
    toolchain = next((a for a in argv if a.startswith("+")), "")
    return run(toolchain)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
