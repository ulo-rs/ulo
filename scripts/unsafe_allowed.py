#!/usr/bin/env python3
"""`unsafe` only where `unsafe-allowed.txt` lists it.

    python3 scripts/unsafe_allowed.py check        # fails on `unsafe` outside the list
    python3 scripts/unsafe_allowed.py miri         # runs Miri over the listed modules' tests
    python3 scripts/unsafe_allowed.py miri --dry-run

The workspace denies `unsafe_code` through `[workspace.lints.rust]`, so the compiler refuses
`unsafe` in any member that inherits the table, unless a module relaxes the lint. `check` fails
when a member does not inherit the table, when the table no longer denies or forbids the lint,
when a file the list does not name relaxes `unsafe_code` or contains `unsafe`, and when a listed
file does not exist or does not opt out with `#![allow(unsafe_code)]`. Comments, string literals
and character literals are not read, so prose mentioning `unsafe` is not a finding.

`miri` runs `cargo miri test` once per listed module: a module of a library or binary target as
that target's tests filtered to the module's path, a file that is a test target as the whole
target. The toolchain is `$MIRI_TOOLCHAIN`, `nightly` when unset.

Standard library only, and Python 3.9, so it runs on a stock runner and a stock macOS.
"""

import json
import os
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
LIST = ROOT / "unsafe-allowed.txt"

OPT_OUT = re.compile(r"#!\[\s*allow\s*\(\s*unsafe_code\s*\)\s*\]")
UNSAFE = re.compile(r"(?<![A-Za-z0-9_])unsafe(?![A-Za-z0-9_])")
LINT = re.compile(r"(?<![A-Za-z0-9_])unsafe_code(?![A-Za-z0-9_])")
# The levels that keep the lint an error; any other naming of it relaxes it.
STRICT = re.compile(r"(?:deny|forbid)\s*\(\s*$")


def listed():
    entries = []
    for number, line in enumerate(LIST.read_text().splitlines(), 1):
        line = line.strip()
        if line and not line.startswith("#"):
            entries.append((number, line))
    return entries


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
    findings = []
    allowed = set()
    for number, entry in listed():
        path = ROOT / entry
        if not path.is_file():
            findings.append(f"unsafe-allowed.txt:{number}: `{entry}` is no file")
            continue
        allowed.add(path.resolve())
        if not OPT_OUT.search(code_only(path.read_text())):
            findings.append(f"{entry}: listed, but does not opt out with `#![allow(unsafe_code)]`")

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


def miri_commands():
    meta = metadata()
    toolchain = os.environ.get("MIRI_TOOLCHAIN", "nightly")
    commands = []
    for _, entry in listed():
        path = (ROOT / entry).resolve()
        package = max(
            (p for p in meta["packages"] if path.is_relative_to(Path(p["manifest_path"]).parent)),
            key=lambda p: len(Path(p["manifest_path"]).parent.parts),
        )
        targets = package["targets"]
        exact = [t for t in targets if Path(t["src_path"]).resolve() == path]
        if exact and "test" in exact[0]["kind"]:
            selector, filter_ = ["--test", exact[0]["name"]], []
        else:
            roots = [t for t in targets if t["kind"][0] in ("lib", "bin") and path.is_relative_to(Path(t["src_path"]).resolve().parent)]
            roots.sort(key=lambda t: t["kind"][0] != "lib")
            if not roots:
                raise SystemExit(f"{entry}: under no library or binary target of {package['name']}")
            root = roots[0]
            selector = ["--lib"] if root["kind"][0] == "lib" else ["--bin", root["name"]]
            module = path.relative_to(Path(root["src_path"]).resolve().parent).with_suffix("")
            parts = list(module.parts)
            if parts[-1] == "mod":
                parts.pop()
            filter_ = ["::".join(parts) + "::"] if parts and path != Path(root["src_path"]).resolve() else []
        commands.append(["cargo", f"+{toolchain}", "miri", "test", "--locked", "-p", package["name"], *selector, "--", *filter_])
    return commands


def miri(dry_run):
    commands = miri_commands()
    if not commands:
        print("unsafe-allowed.txt lists no module: Miri has nothing to run")
        return 0
    failed = 0
    for command in commands:
        print("$ " + " ".join(command), flush=True)
        if not dry_run:
            failed |= subprocess.run(command, cwd=ROOT).returncode != 0
    return 1 if failed else 0


if __name__ == "__main__":
    if len(sys.argv) >= 2 and sys.argv[1] == "check":
        sys.exit(check())
    if len(sys.argv) >= 2 and sys.argv[1] == "miri":
        sys.exit(miri("--dry-run" in sys.argv[2:]))
    print("usage: unsafe_allowed.py check | miri [--dry-run]", file=sys.stderr)
    sys.exit(2)
