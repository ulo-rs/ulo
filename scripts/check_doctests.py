"""Fails when a doc example is skipped by rustdoc without a stated reason.

The rule for every doc example in the crates: a compiled doctest unless it is marked with a reason.
A fenced block in a `///` or `//!` doc comment of `crates/*/src/**/*.rs`, or in `crates/*/README.md`,
whose info string carries `ignore` (rustdoc then skips it, so nothing compiles it) must say why,
either as a `// reason: ...` comment line within the first ten lines of the block (`# // reason: ...`
when the line is to be hidden from the rendered page) or as `reason: ...` on the doc line directly
above the fence. A reason is at least three words, so `todo` or `x` is none. An info
string rustdoc does not read as Rust although Rust was meant (`rs`, `Rust`, `IGNORE`, `ignored`)
is skipped the same way, silently, and is held to the same rule. `ignore = "..."` is not rustdoc
syntax: it warns `unexpected character` and drops the block from the test list, so the fence form
carries no reason. Blocks of any other language, blocks that compile (`rust`, `no_run`,
`compile_fail`, no tag) and blocks marked `text` are not touched. A README is scanned for `ignore`
blocks only: no crate includes its README into its docs, so nothing compiles a README's `rust`
block either.

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
HEAD = 10
DOC = re.compile(r"^\s*//[/!] ?(.*)$")
REASON = re.compile(r"^\s*(?:#\s*)?(?://\s*)?reason:\s*(.*)$", re.IGNORECASE)


def reasoned(line):
    m = REASON.match(line)
    if not m:
        return False
    text = m.group(1).strip()
    return len(re.findall(r"[A-Za-z][\w'.-]*", text)) >= 3


def ignored(info):
    tags = [t for t in re.split(r"[,\s]+", info.strip()) if t]
    # rustdoc matches its tags case-sensitively and reads an unknown tag as another language, so
    # `IGNORE`, `ignored` and `Rust` skip the block as `ignore` does, with no warning.
    return any(t.lower().startswith("ignore") or (t.lower() in ("rust", "rs") and t != "rust") for t in tags)


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
        # CommonMark reads a fence line carrying an info string inside a block as content, not as a
        # close. Reading it as the next block's opener instead can only report a block rustdoc
        # does not see, never miss one it does.
        reopened = j < len(lines) and FENCE.match(lines[j]) and FENCE.match(lines[j]).group(3).strip()
        i = j if reopened else j + 1


def violations(lines):
    for number, info, body, above in blocks(lines):
        if not ignored(info):
            continue
        head = body[:HEAD]
        if any(reasoned(l) for l in head) or reasoned(above):
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
    eleven = "".join(f"//! l{n}();\n" for n in range(10))
    bad = {
        "crates/a/src/lib.rs": "//! ```ignore\n//! let x = 1;\n//! ```\n",
        "crates/b/src/lib.rs": "/// ```rust,ignore\n/// f();\n/// ```\nfn f() {}\n",
        "crates/c/README.md": "```rust,ignore\nf();\n```\n",
        "crates/e/src/lib.rs": "//! ```ignore\n//! // reason: needs a live broker\n//!\n//! ```ignore\n//! b();\n//! ```\n",
        "crates/f/src/lib.rs": "//! ```ignore\n//! # // reason: todo\n//! ```\n",
        "crates/g/src/lib.rs": "//! ```ignore\n//! f(); // reason: a trailing comment on code\n//! ```\n",
        "crates/h/src/lib.rs": "//! ```ignore\n" + eleven + "//! // reason: below the first ten lines\n//! ```\n",
        "crates/i/src/lib.rs": "//! ```ignore = \"needs a live broker\"\n//! f();\n//! ```\n",
        "crates/j/src/lib.rs": "//! ```rs\n//! f();\n//! ```\n",
        "crates/k/src/lib.rs": "//! ```IGNORE\n//! f();\n//! ```\n",
    }
    planted = {("crates/a/src/lib.rs", 1), ("crates/b/src/lib.rs", 1), ("crates/c/README.md", 1),
               ("crates/e/src/lib.rs", 4), ("crates/f/src/lib.rs", 1), ("crates/g/src/lib.rs", 1),
               ("crates/h/src/lib.rs", 1), ("crates/i/src/lib.rs", 1), ("crates/j/src/lib.rs", 1),
               ("crates/k/src/lib.rs", 1)}
    good = {
        "crates/a/src/lib.rs": "//! ```ignore\n//! // reason: needs a network\n//! f();\n//! ```\n//! ```text\n//! plain\n//! ```\n//! ```no_run\n//! f();\n//! ```\n",
        "crates/b/src/lib.rs": "/// reason: needs a live broker\n/// ```ignore\n/// f();\n/// ```\n",
        "crates/d/src/lib.rs": "//! ```ignore\n//! a();\n//! b();\n//! c();\n//! # // reason: names the caller's types\n//! ```\n",
        "crates/c/README.md": "```rust\nf();\n```\n```toml\na = 1\n```\n",
    }
    for name, files, expected in (("planted", bad, planted), ("clean", good, set())):
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            for rel, text in files.items():
                (root / rel).parent.mkdir(parents=True, exist_ok=True)
                (root / rel).write_text(text, encoding="utf-8")
            found = {(path.relative_to(root).as_posix(), number) for path, number, _ in scan(root)}
            if found != expected:
                print(f"self-test failed on the {name} tree: missed {sorted(expected - found)}, extra {sorted(found - expected)}", file=sys.stderr)
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
