"""Fails when a doc example is skipped by rustdoc without a stated reason.

The rule for every doc example in the crates: a compiled doctest unless it is marked with a reason.
A fenced block in a `///` or `//!` doc comment of `crates/*/src/**/*.rs`, or in `crates/*/README.md`,
whose info string carries `ignore` (rustdoc then skips it, so nothing compiles it) must say why,
either as a `// reason: ...` line at the top of the block (`# // reason: ...` when the line is to be
hidden from the rendered page) or as `reason: ...` on the doc line directly above the fence.
Blocks of any other language, blocks that compile (`rust`, `no_run`, `compile_fail`, no tag) and
blocks marked `text` are not touched.

    python3 scripts/check_doctests.py [ROOT]
    python3 scripts/check_doctests.py --self-test

`ROOT` is the directory holding `crates/` (default: the repository root). The real check runs the
self-test first, so a scanner that has stopped matching a known violation fails rather than passing
every tree. Exit status 1 on a hit, 0 on none. Standard library only.
"""

import pathlib
import re
import sys
import tempfile

FENCE = re.compile(r"^(\s*)(`{3,}|~{3,})\s*([^`]*)$")
DOC = re.compile(r"^\s*//[/!] ?(.*)$")
REASON = re.compile(r"(?://|#\s*//)\s*reason:\s*\S|^\s*reason:\s*\S", re.IGNORECASE)


def ignored(info):
    tags = [t for t in re.split(r"[,\s]+", info.strip()) if t]
    return any(t == "ignore" or t.startswith("ignore-") or t.startswith("ignore=") for t in tags)


def blocks(lines):
    """Yields (line number of the fence, info string, body lines, line above the fence)."""
    i = 0
    while i < len(lines):
        m = FENCE.match(lines[i])
        if not m:
            i += 1
            continue
        marker, info = m.group(2), m.group(3)
        above = lines[i - 1] if i else ""
        j = i + 1
        while j < len(lines) and not lines[j].strip().startswith(marker[0] * len(marker)):
            j += 1
        yield i + 1, info, lines[i + 1 : j], above
        i = j + 1


def violations(lines):
    for number, info, body, above in blocks(lines):
        if not ignored(info):
            continue
        head = [l for l in body[:3]]
        if any(REASON.search(l) for l in head) or REASON.search(above):
            continue
        yield number, info.strip()


def doc_lines(text):
    """The doc-comment lines of a Rust source, comment markers stripped, other lines blanked so
    line numbers stay those of the file and a fence never spans code."""
    out = []
    for line in text.splitlines():
        m = DOC.match(line)
        out.append(m.group(1) if m else "")
    return out


def scan(root):
    hits = []
    for path in sorted(root.glob("crates/*/src/**/*.rs")):
        for number, info in violations(doc_lines(path.read_text(encoding="utf-8"))):
            hits.append((path, number, info))
    for path in sorted(root.glob("crates/*/README.md")):
        for number, info in violations(path.read_text(encoding="utf-8").splitlines()):
            hits.append((path, number, info))
    return hits


def self_test():
    bad = {
        "crates/a/src/lib.rs": "//! ```ignore\n//! let x = 1;\n//! ```\n",
        "crates/b/src/lib.rs": "/// ```rust,ignore\n/// f();\n/// ```\nfn f() {}\n",
        "crates/c/README.md": "```rust,ignore\nf();\n```\n",
    }
    good = {
        "crates/a/src/lib.rs": "//! ```ignore\n//! // reason: needs a network\n//! f();\n//! ```\n//! ```text\n//! plain\n//! ```\n//! ```no_run\n//! f();\n//! ```\n",
        "crates/b/src/lib.rs": "/// reason: needs a live broker\n/// ```ignore\n/// f();\n/// ```\n",
        "crates/c/README.md": "```rust\nf();\n```\n```toml\na = 1\n```\n",
    }
    for name, files, expected in (("planted", bad, len(bad)), ("clean", good, 0)):
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            for rel, text in files.items():
                (root / rel).parent.mkdir(parents=True, exist_ok=True)
                (root / rel).write_text(text, encoding="utf-8")
            found = len(scan(root))
            if found != expected:
                print(f"self-test failed on the {name} tree: {found} hits, expected {expected}", file=sys.stderr)
                return False
    return True


def main(argv):
    if "--self-test" in argv:
        ok = self_test()
        print("self-test ok" if ok else "self-test FAILED")
        return 0 if ok else 1
    if not self_test():
        return 1
    root = pathlib.Path(next((a for a in argv[1:] if not a.startswith("-")), pathlib.Path(__file__).resolve().parent.parent))
    hits = scan(root)
    for path, number, info in hits:
        print(f"{path.relative_to(root)}:{number}: ```{info} is skipped by rustdoc and gives no `reason:`")
    if hits:
        print(f"{len(hits)} doc example(s) neither compiled nor marked with a reason", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
