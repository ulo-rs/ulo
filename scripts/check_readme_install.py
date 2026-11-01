"""Fails when a README's Install section names a `ulo` version other than the workspace's.

The workspace version is `[workspace.package] version` in the root `Cargo.toml`. The Install
section (`## Install` up to the next `## ` heading) of the root `README.md` and of
`crates/ulo/README.md` is read for every claim about a version:

- a git dependency, `git = ".."` or `git = '..'` in a manifest or `--git` on a `cargo add`, which
  is refused outright: the released version is on crates.io, so nobody is sent to a git tag;
- a `tag = "v.."`, which must be `v` plus the workspace version;
- a `ulo` or `ulo-*` dependency's version, `ulo = "x.y.z"` or `ulo = { version = "x.y.z", .. }`,
  which must be the workspace version, a leading `=`, `^` or `~` aside; a partial `"x.y"` or `"x"`
  is read too, and differs from it;
- `cargo add ulo[-*]@x.y.z`, which must be the workspace version, a partial one read as above;
- a backticked tag in prose, `` `vx.y.z` ``, which must be `v` plus the workspace version.

A README with no Install section, or an Install section that names no version, fails too: a check
that found nothing to compare would pass for the wrong reason.

    python3 scripts/check_readme_install.py [ROOT]
    python3 scripts/check_readme_install.py --self-test

`ROOT` is the repository root (default: the directory above `scripts/`). The real check runs the
self-test first. Exit status 1 on a mismatch, 0 on none. Standard library only, Python 3.9.
"""

import pathlib
import re
import sys
import tempfile

READMES = ["README.md", "crates/ulo/README.md"]
VERSION = r"[0-9]+\.[0-9]+\.[0-9]+(?:[-+][0-9A-Za-z.+-]+)?"
# What a dependency or `cargo add` may write: cargo reads `"0.1"` as `^0.1`, a claim about the
# version as much as `"0.1.0"` is.
REQUIREMENT = r"[0-9]+(?:\.[0-9]+){0,2}(?:[-+][0-9A-Za-z.+-]+)?"
GIT = re.compile(r"""\bgit\s*=\s*["']|--git\b""")
CLAIMS = [
    ("a git tag", re.compile(r'\btag\s*=\s*"(v' + VERSION + r')"'), True),
    ("a dependency version", re.compile(r'^\s*ulo[\w-]*\s*=\s*"[=^~]?(' + REQUIREMENT + r')"', re.M), False),
    ("a dependency version", re.compile(r'^\s*ulo[\w-]*\s*=\s*\{[^}]*\bversion\s*=\s*"[=^~]?(' + REQUIREMENT + r')"', re.M), False),
    ("a `cargo add` version", re.compile(r"\bcargo\s+add\s+ulo[\w-]*@(" + REQUIREMENT + r")"), False),
    ("a tag in prose", re.compile(r"`(v" + VERSION + r")`"), True),
]


def workspace_version(root):
    text = (root / "Cargo.toml").read_text()
    section = re.search(r"^\[workspace\.package\]\s*$(.*?)(?=^\[|\Z)", text, re.M | re.S)
    found = re.search(r'^version\s*=\s*"([^"]+)"', section.group(1), re.M) if section else None
    if not found:
        raise SystemExit("check_readme_install.py: no `[workspace.package] version` in Cargo.toml")
    return found.group(1)


def install_section(text):
    start = re.search(r"^## Install\s*$", text, re.M)
    if not start:
        return None
    rest = text[start.end():]
    end = re.search(r"^## ", rest, re.M)
    return rest[: end.start()] if end else rest


def claims(section):
    """Every `(what, version as written, line number within the section)`."""
    found = []
    for what, pattern, tagged in CLAIMS:
        for m in pattern.finditer(section):
            found.append((what, m.group(1), section.count("\n", 0, m.start(1)) + 1, tagged))
    return sorted(found, key=lambda c: c[2])


def check(root):
    """Every `(readme, message)` problem under `root`, plus the claims read, as `(readme, claim)`."""
    version = workspace_version(root)
    problems, read = [], []
    for name in READMES:
        path = root / name
        if not path.is_file():
            problems.append((name, "missing"))
            continue
        section = install_section(path.read_text())
        if section is None:
            problems.append((name, "has no `## Install` section"))
            continue
        for m in GIT.finditer(section):
            problems.append((name, f"Install line {section.count(chr(10), 0, m.start()) + 1}: a git dependency; name the crates.io version"))
        found = claims(section)
        if not found:
            problems.append((name, "Install section names no version, so nothing was compared"))
        for what, written, line, tagged in found:
            want = "v" + version if tagged else version
            read.append((name, f"{what} `{written}` (Install line {line})"))
            if written != want:
                problems.append((name, f"Install line {line}: {what} `{written}`, the workspace version is `{version}` (want `{want}`)"))
    return problems, read


GOOD = """# x

## Install

The `v1.2.3` tag is the current release.

```toml
ulo = "1.2.3"
ulo-http = "1.2.3"
ulo-net = { version = "=1.2.3", default-features = false }
serde = "9.9.9"
```

or `cargo add ulo@1.2.3`.

## Quick start

ulo = "0.0.1" below the Install section is not read.
"""
# Each variant of GOOD must be refused.
BAD = {
    "git install": GOOD.replace('ulo = "1.2.3"', 'ulo = { git = "https://example.test/ulo", tag = "v1.2.3" }'),
    "git tag": GOOD.replace('ulo = "1.2.3"', 'ulo = { git = "https://example.test/ulo", tag = "v1.2.2" }'),
    "prose tag": GOOD.replace("`v1.2.3` tag", "`v1.2.4` tag"),
    "bare version": GOOD.replace('ulo-http = "1.2.3"', 'ulo-http = "1.2.2"'),
    "table version": GOOD.replace("=1.2.3", "=1.3.0"),
    "cargo add": GOOD.replace("ulo@1.2.3", "ulo@1.2.0"),
    "literal-string git": GOOD.replace('ulo = "1.2.3"', "ulo = { git = 'https://example.test/ulo', tag = 'v1.2.3' }"),
    "cargo add --git": GOOD.replace("cargo add ulo@1.2.3", "cargo add ulo@1.2.3 --git https://example.test/ulo"),
    "partial version": GOOD.replace('ulo-http = "1.2.3"', 'ulo-http = "1.2"'),
    "no section": GOOD.replace("## Install", "## Setup"),
    "no version": "# x\n\n## Install\n\nrun cargo add ulo\n",
}


def fixture(root, version, text):
    (root / "crates" / "ulo").mkdir(parents=True, exist_ok=True)
    (root / "Cargo.toml").write_text(f'[workspace]\nmembers = []\n\n[workspace.package]\nversion = "{version}"\n\n[workspace.dependencies]\nulo = "0.0.1"\n')
    for name in READMES:
        (root / name).write_text(text)


def self_test():
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        fixture(root, "1.2.3", GOOD)
        problems, read = check(root)
        if problems or len(read) != 2 * 5:
            sys.exit(f"check_readme_install.py: self-test: the matching fixture read {len(read)} claims and gave {problems}")
        fixture(root, "1.2.4", GOOD)
        if len(check(root)[0]) != 2 * 5:
            sys.exit("check_readme_install.py: self-test: a workspace version bump did not flag every claim")
        prerelease = GOOD.replace("1.2.3", "1.2.3-alpha.2")
        fixture(root, "1.2.3-alpha.2", prerelease)
        problems, read = check(root)
        if problems or len(read) != 2 * 5:
            sys.exit(f"check_readme_install.py: self-test: the prerelease fixture read {len(read)} claims and gave {problems}")
        fixture(root, "1.2.3-alpha.2", prerelease.replace('ulo-http = "1.2.3-alpha.2"', 'ulo-http = "1.2.3-alpha.1"'))
        if not check(root)[0]:
            sys.exit("check_readme_install.py: self-test: the prerelease mismatch was not refused")
        for label, text in BAD.items():
            fixture(root, "1.2.3", text)
            if not check(root)[0]:
                sys.exit(f"check_readme_install.py: self-test: the {label} mismatch was not refused")


def main(argv):
    if argv == ["--self-test"]:
        self_test()
        print("check_readme_install.py: self-test passed")
        return 0
    root = pathlib.Path(argv[0]) if argv else pathlib.Path(__file__).resolve().parent.parent
    self_test()
    problems, read = check(root)
    for name, claim in read:
        print(f"{name}: {claim}")
    for name, message in problems:
        print(f"{name}: {message}", file=sys.stderr)
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
