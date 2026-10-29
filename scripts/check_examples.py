"""Fails when an example under `examples/` reaches past the public API.

An example is what a user would write, so it names no path through a doc-hidden module and no
doc-hidden item. Every doc-hidden name in the crates an example can reach starts with two
underscores (`__private`, `__handler`, `__enhancer_specs`, the macro probes' `__ulo_*` methods),
so the check refuses any identifier that does, comments included, and any `doc(..)` attribute that
lists `hidden`, however it is spaced or combined. It reads the Rust sources and manifests of every
`examples/NN-name/` package and of `examples/support`, the helper their tests share.

    python3 scripts/check_examples.py [ROOT]
    python3 scripts/check_examples.py --self-test

`ROOT` is the directory holding `examples/` (default: the repository root). The real check runs the
self-test first, so a pattern that has stopped matching a known violation fails rather than passing
every tree. Exit status 1 on a hit, 0 on none. Standard library only.
"""

import pathlib
import re
import sys
import tempfile

FORBIDDEN = [
    (re.compile(r"(?<![A-Za-z0-9_])(?:r#)?__[A-Za-z]\w*"), "a doc-hidden name (two leading underscores)"),
    (re.compile(r"\bdoc\s*\([^)]*\bhidden\b"), "a `#[doc(hidden)]` attribute"),
]
EXAMPLE = re.compile(r"^\d{2}-[a-z0-9-]+$")
SHARED = {"support"}

# Each line must be refused; the last one must not.
SAMPLES = [
    "use ulo_http::__private::HttpHandler;",
    "fn f() { ulo::__private::x(); }",
    "use ulo::{__private as p};",
    "use ulo_http::r#__private;",
    "fn f() { ulo::__http::into_sse(); }",
    "ulo_macros::__handler!();",
    "let c = (&&PathProbe::<P>::new()).__ulo_check();",
    "#[doc(hidden)]",
    "#[doc( hidden )]",
    "# [doc(hidden)]",
    "#[doc(alias = \"y\", hidden)]",
    "#[cfg_attr(all(), doc(hidden))]",
]
CLEAN = "let x = a__b; // snake_case with a double underscore inside a name is not a hidden path"


def directories(root):
    examples = root / "examples"
    if not examples.is_dir():
        return []
    return [d for d in sorted(examples.iterdir()) if d.is_dir() and (EXAMPLE.match(d.name) or d.name in SHARED)]


def scan(root):
    """Every `(file, line number, what)` hit under the scanned directories of `root/examples/`."""
    hits = []
    for directory in directories(root):
        for file in sorted(directory.rglob("*")):
            if file.suffix not in (".rs", ".toml") or "target" in file.parts:
                continue
            for number, line in enumerate(file.read_text(encoding="utf-8").splitlines(), 1):
                for pattern, what in FORBIDDEN:
                    if pattern.search(line):
                        hits.append((file, number, what))
    return hits


def self_test():
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        for name in ("99-sample", "support"):
            src = root / "examples" / name / "src"
            src.mkdir(parents=True)
            (src / "lib.rs").write_text("\n".join(SAMPLES + [CLEAN]) + "\n", encoding="utf-8")
        lines = {(h[0].parent.parent.name, h[1]) for h in scan(root)}
    missed = [(name, s) for name in ("99-sample", "support") for i, s in enumerate(SAMPLES, 1) if (name, i) not in lines]
    flagged_clean = [name for name in ("99-sample", "support") if (name, len(SAMPLES) + 1) in lines]
    if missed or flagged_clean:
        print(f"self-test: missed {missed}, flagged the clean line in {flagged_clean}", file=sys.stderr)
        return False
    return True


def main(argv):
    if not self_test():
        return 1
    if argv[1:] == ["--self-test"]:
        print("self-test passed")
        return 0
    root = pathlib.Path(argv[1]) if len(argv) > 1 else pathlib.Path(__file__).resolve().parent.parent
    hits = scan(root)
    for file, number, what in hits:
        print(f"{file.relative_to(root)}:{number}: {what}")
    print(f"{len(directories(root))} directories scanned, {len(hits)} findings")
    return 1 if hits else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
