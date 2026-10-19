#!/usr/bin/env python3
"""`unsafe` only where `unsafe-allowed.txt` lists it.

    python3 scripts/unsafe_allowed.py check        # fails on `unsafe` outside the list
    python3 scripts/unsafe_allowed.py miri         # runs Miri over the pure-Rust entries' tests
    python3 scripts/unsafe_allowed.py miri --dry-run

The workspace denies `unsafe_code` through `[workspace.lints.rust]`, so the compiler refuses
`unsafe` in any member that inherits the table, unless a module relaxes the lint. `check` fails
when a member does not inherit the table, when the table no longer denies or forbids the lint,
when a file the list does not name relaxes `unsafe_code` or contains `unsafe`, when a listed
file does not exist or does not opt out with `#![allow(unsafe_code)]`, and when an entry's kind is
missing or unknown, a `syscall` entry gives no reason or one of whitespace alone, a `pure-rust`
entry has anything after the word, or a path is listed twice. Comments, string literals and
character literals are not read, so prose mentioning `unsafe` is not a finding.

Both commands first run the list's rules over `scripts/unsafe_allowed_selftest.txt`, whose lines
each break one rule, and fail unless every one of them is refused with that rule's finding and
the file's valid lines pass. The check cannot then report a list clean under a rule it no longer
enforces.

`miri` runs `cargo miri test` once per `pure-rust` entry: a module of a library or binary target
as that target's tests filtered to the module's path, a file that is a test target as the whole
target. It counts the entry's tests Miri executed, from libtest's per-test lines, and fails when
Miri fails or executes none of them, ignored tests not counting. It prints each `syscall` entry
with its reason and runs nothing for it. The toolchain is `$MIRI_TOOLCHAIN`, `nightly` when unset.

Standard library only, and Python 3.9, so it runs on a stock runner and a stock macOS.
"""

import json
import os
import re
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from ran_a_test import ran_a_test  # noqa: E402  the definition `template_matrix.py` shares

ROOT = Path(__file__).resolve().parent.parent
LIST = ROOT / "unsafe-allowed.txt"
SELFTEST = ROOT / "scripts" / "unsafe_allowed_selftest.txt"

OPT_OUT = re.compile(r"#!\[\s*allow\s*\(\s*unsafe_code\s*\)\s*\]")
UNSAFE = re.compile(r"(?<![A-Za-z0-9_])unsafe(?![A-Za-z0-9_])")
LINT = re.compile(r"(?<![A-Za-z0-9_])unsafe_code(?![A-Za-z0-9_])")
# The levels that keep the lint an error; any other naming of it relaxes it.
STRICT = re.compile(r"(?:deny|forbid)\s*\(\s*$")


# Miri executes the entry's `unsafe`; the entry takes no reason.
PURE = "pure-rust"
# The entry's `unsafe` calls into the operating system, which Miri cannot execute; the rest of the
# line says what Miri stops at.
SYSCALL = "syscall"


class Entry:
    def __init__(self, number, path, kind, reason):
        self.number, self.path, self.kind, self.reason = number, path, kind, reason


def parse(text):
    """Every entry of a list in the list's grammar, each `kind` and `reason` as found, which
    `entry_findings` judges. Whitespace is Python's, Unicode's included."""
    entries = []
    for number, line in enumerate(text.splitlines(), 1):
        line = line.strip()
        if line and not line.startswith("#"):
            fields = line.split(None, 2)
            kind = fields[1] if len(fields) > 1 else None
            reason = fields[2].strip() if len(fields) > 2 else ""
            entries.append(Entry(number, fields[0], kind, reason))
    return entries


def listed():
    return parse(LIST.read_text())


def entry_findings(entries):
    """Each entry that breaks the list's grammar, as `(line number, finding)`."""
    findings = []
    seen = {}
    for entry in entries:
        if entry.path in seen:
            findings.append((entry.number, f"`{entry.path}` is listed again, first at line {seen[entry.path]}"))
        seen.setdefault(entry.path, entry.number)
        if entry.kind is None:
            findings.append((entry.number, f"`{entry.path}` has no kind; write `{PURE}`, or `{SYSCALL}` and what Miri stops at"))
        elif entry.kind == PURE and entry.reason:
            findings.append((entry.number, f"a `{PURE}` entry takes no reason, since Miri runs its tests: `{entry.reason}`"))
        elif entry.kind == SYSCALL and not entry.reason:
            findings.append((entry.number, f"a `{SYSCALL}` entry gives the reason Miri cannot execute it after the kind"))
        elif entry.kind not in (PURE, SYSCALL):
            findings.append((entry.number, f"`{entry.kind}` is no kind; write `{PURE}` or `{SYSCALL}`"))
    return findings


def located(findings, name="unsafe-allowed.txt"):
    return [f"{name}:{number}: {finding}" for number, finding in findings]


# In the self-test file: the finding the next entry must draw, and the token standing for the
# whitespace of a plant, which an editor stripping trailing whitespace would otherwise remove.
EXPECT = "# expect: "
WHITESPACE = "<whitespace>"


def selftest():
    """The list's rules run over `SELFTEST`: a failure for each planted line not refused with the
    finding its `# expect:` names, or refused with another as well, and for each valid line
    refused at all. Prints how many of each it saw when nothing fails."""
    lines = SELFTEST.read_text().replace(WHITESPACE, " \t\u00a0 ").splitlines()
    expected = {}
    pending = None
    for number, line in enumerate(lines, 1):
        stripped = line.strip()
        if stripped.startswith(EXPECT):
            pending = stripped[len(EXPECT):]
        elif stripped and not stripped.startswith("#"):
            if pending is not None:
                expected[number] = pending
            pending = None
    entries = parse("\n".join(lines))
    found = {}
    for number, finding in entry_findings(entries):
        found.setdefault(number, []).append(finding)
    name = SELFTEST.relative_to(ROOT)
    failures = []
    for entry in entries:
        drawn = found.get(entry.number, [])
        want = expected.get(entry.number)
        if want is None and drawn:
            failures.append(f"self-test: {name}:{entry.number}: a valid line was refused: {'; '.join(drawn)}")
        elif want is not None and not any(want in finding for finding in drawn):
            failures.append(f"self-test: {name}:{entry.number}: a planted line was not refused with `{want}`; found: {'; '.join(drawn) or 'nothing'}")
        elif want is not None and len(drawn) > 1:
            failures.append(f"self-test: {name}:{entry.number}: a planted line drew more than its own finding: {'; '.join(drawn)}")
    if not expected:
        failures.append(f"self-test: {name} plants no line")
    if not failures:
        print(f"self-test: {len(expected)} planted lines refused, {len(entries) - len(expected)} valid lines accepted")
    return failures


def metadata():
    out = subprocess.run(
        ["cargo", "metadata", "--no-deps", "--format-version", "1"],
        cwd=ROOT, check=True, capture_output=True, text=True,
    ).stdout
    return json.loads(out)


def code_only(text):
    """`text` with every comment and string or character literal blanked, newlines kept."""
    out = []
    i, n = 0, len(text)

    def blank(segment):
        out.append(re.sub(r"[^\n]", " ", segment))

    while i < n:
        c = text[i]
        if text.startswith("//", i):
            end = text.find("\n", i)
            end = n if end == -1 else end
            blank(text[i:end])
            i = end
        elif text.startswith("/*", i):
            depth, j = 1, i + 2
            while j < n and depth:
                if text.startswith("/*", j):
                    depth, j = depth + 1, j + 2
                elif text.startswith("*/", j):
                    depth, j = depth - 1, j + 2
                else:
                    j += 1
            blank(text[i:j])
            i = j
        elif re.match(r"(?:b|c)?r#*\"", text[i:]) and (i == 0 or not (text[i - 1].isalnum() or text[i - 1] == "_")):
            m = re.match(r"(?:b|c)?r(#*)\"", text[i:])
            close = "\"" + m.group(1)
            end = text.find(close, i + m.end())
            end = n if end == -1 else end + len(close)
            blank(text[i:end])
            i = end
        elif c == "\"" or (c in "bc" and text.startswith("\"", i + 1) and (i == 0 or not (text[i - 1].isalnum() or text[i - 1] == "_"))):
            j = i + (1 if c == "\"" else 2)
            while j < n and text[j] != "\"":
                j += 2 if text[j] == "\\" else 1
            blank(text[i:j + 1])
            i = j + 1
        elif c == "'":
            # A character literal, `'x'` or `'\n'`; otherwise a lifetime or a label, which stays.
            if text.startswith("\\", i + 1):
                end = text.find("'", i + 2)
                end = n if end == -1 else end + 1
                blank(text[i:end])
                i = end
            elif i + 2 < n and text[i + 2] == "'":
                blank(text[i:i + 3])
                i += 3
            else:
                out.append(c)
                i += 1
        else:
            out.append(c)
            i += 1
    return "".join(out)


def line_of(text, offset):
    return text.count("\n", 0, offset) + 1


def inherits_workspace_lints(manifest):
    section = None
    for raw in manifest.read_text().splitlines():
        line = raw.split("#", 1)[0].strip()
        if line.startswith("["):
            section = line
        elif section == "[lints]" and re.fullmatch(r"workspace\s*=\s*true", line):
            return True
    return False


def workspace_denies():
    section = None
    for raw in (ROOT / "Cargo.toml").read_text().splitlines():
        line = raw.split("#", 1)[0].strip()
        if line.startswith("["):
            section = line
        elif section == "[workspace.lints.rust]" and re.fullmatch(r"unsafe_code\s*=\s*\"(deny|forbid)\"", line):
            return True
    return False


def check():
    failures = selftest()
    if failures:
        for failure in failures:
            print(failure)
        return 1
    entries = listed()
    findings = located(entry_findings(entries))
    allowed = set()
    for entry in entries:
        path = ROOT / entry.path
        if not path.is_file():
            findings.append(f"unsafe-allowed.txt:{entry.number}: `{entry.path}` is no file")
            continue
        allowed.add(path.resolve())
        if not OPT_OUT.search(code_only(path.read_text())):
            findings.append(f"{entry.path}: listed, but does not opt out with `#![allow(unsafe_code)]`")

    if not workspace_denies():
        findings.append("Cargo.toml: `[workspace.lints.rust]` does not set `unsafe_code` to \"deny\" or \"forbid\"")

    scanned = 0
    for package in metadata()["packages"]:
        manifest = Path(package["manifest_path"])
        if not inherits_workspace_lints(manifest):
            findings.append(f"{manifest.relative_to(ROOT)}: no `[lints] workspace = true`, so the workspace's `unsafe_code` level does not reach `{package['name']}`")
        for path in sorted(manifest.parent.rglob("*.rs")):
            if "target" in path.relative_to(manifest.parent).parts:
                continue
            scanned += 1
            if path.resolve() in allowed:
                continue
            text = path.read_text()
            code = code_only(text)
            where = path.relative_to(ROOT)
            for m in UNSAFE.finditer(code):
                findings.append(f"{where}:{line_of(code, m.start())}: `unsafe` outside the modules unsafe-allowed.txt lists")
            for m in LINT.finditer(code):
                if not STRICT.search(code[:m.start()]):
                    findings.append(f"{where}:{line_of(code, m.start())}: `unsafe_code` relaxed outside the modules unsafe-allowed.txt lists")

    for finding in findings:
        print(finding)
    print(f"{scanned} files scanned, {len(allowed)} listed, {len(findings)} findings")
    return 1 if findings else 0


def miri_command(meta, toolchain, entry):
    """The `cargo miri test` command for `entry`, and the prefix its tests' names carry, `None`
    for a test target, whose every test is the entry's."""
    path = (ROOT / entry.path).resolve()
    package = max(
        (p for p in meta["packages"] if path.is_relative_to(Path(p["manifest_path"]).parent)),
        key=lambda p: len(Path(p["manifest_path"]).parent.parts),
    )
    targets = package["targets"]
    exact = [t for t in targets if Path(t["src_path"]).resolve() == path]
    if exact and "test" in exact[0]["kind"]:
        selector, prefix = ["--test", exact[0]["name"]], None
    else:
        roots = [t for t in targets if t["kind"][0] in ("lib", "bin") and path.is_relative_to(Path(t["src_path"]).resolve().parent)]
        roots.sort(key=lambda t: t["kind"][0] != "lib")
        if not roots:
            raise SystemExit(f"{entry.path}: under no library or binary target of {package['name']}")
        root = roots[0]
        selector = ["--lib"] if root["kind"][0] == "lib" else ["--bin", root["name"]]
        module = path.relative_to(Path(root["src_path"]).resolve().parent).with_suffix("")
        parts = list(module.parts)
        if parts[-1] == "mod":
            parts.pop()
        prefix = "::".join(parts) + "::" if parts and path != Path(root["src_path"]).resolve() else None
    command = ["cargo", f"+{toolchain}", "miri", "test", "--locked", "-p", package["name"], *selector, "--"]
    if prefix:
        command.append(prefix)
    return command, prefix


def miri(dry_run):
    failures = selftest()
    if failures:
        for failure in failures:
            print(failure)
        return 1
    entries = listed()
    findings = located(entry_findings(entries))
    if findings:
        for finding in findings:
            print(finding)
        return 1
    meta = metadata()
    toolchain = os.environ.get("MIRI_TOOLCHAIN", "nightly")
    for entry in entries:
        if entry.kind == SYSCALL:
            print(f"skipped, a system call Miri cannot execute: {entry.path}: {entry.reason}")
    pure = [entry for entry in entries if entry.kind == PURE]
    if not pure:
        print("unsafe-allowed.txt lists no pure-Rust entry: Miri has nothing to run")
        return 0
    failures = []
    for entry in pure:
        command, prefix = miri_command(meta, toolchain, entry)
        print("$ " + " ".join(command), flush=True)
        if dry_run:
            continue
        ran = 0
        process = subprocess.Popen(command, cwd=ROOT, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
        for line in process.stdout:
            sys.stdout.write(line)
            if ran_a_test(line, prefix):
                ran += 1
        sys.stdout.flush()
        code = process.wait()
        print(f"{entry.path}: Miri executed {ran} of its tests, exit {code}", flush=True)
        if code != 0:
            failures.append(f"{entry.path}: Miri failed, exit {code}")
        elif ran == 0:
            failures.append(f"{entry.path}: Miri executed none of its tests; a pure-Rust entry needs at least one that runs under Miri")
    for failure in failures:
        print(failure)
    return 1 if failures else 0


if __name__ == "__main__":
    if len(sys.argv) >= 2 and sys.argv[1] == "check":
        sys.exit(check())
    if len(sys.argv) >= 2 and sys.argv[1] == "miri":
        sys.exit(miri("--dry-run" in sys.argv[2:]))
    print("usage: unsafe_allowed.py check | miri [--dry-run]", file=sys.stderr)
    sys.exit(2)
