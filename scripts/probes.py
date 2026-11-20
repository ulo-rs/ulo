#!/usr/bin/env python3
"""Builds the compiler probes of `docs/experiments/di-redesign/probes/` with the toolchain `cargo`
resolves, and holds each to what its name promises.

    python3 scripts/probes.py                    # every probe, with the default toolchain
    python3 scripts/probes.py +1.88              # the same through `cargo +1.88`
    python3 scripts/probes.py --self-test        # the verdicts, run against planted probes

The probes record what rustc does with the language features the design leans on: a trait object
coerced from a closure, an `on_unimplemented` note, a coherence overlap. They are the one place the
floor toolchain's behaviour is observed directly, since `tests/ui/*.stderr` records one compiler's
wording and is checked on that compiler alone.

A probe whose file name ends `_fails` must not compile, and every error code its output prints must
be one its header comment names (`E0277`): an error the probe was not written for, a typo's `E0425`
among them, fails it beside the one it was. An error rustc prints without a code asks only for the
failure. Any other probe must compile. Every probe is built, so a failure names the probe and the compiler's own output rather
than stopping at the first. The crate is outside the workspace and has no dependencies, so a
build touches nothing else.

Standard library only, and Python 3.9.
"""

import os
import re
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
PROBES = ROOT / "docs" / "experiments" / "di-redesign" / "probes"

CODE = re.compile(r"\bE0\d{3}\b")
ERROR_CODE = re.compile(r"error\[(E0\d{3})\]")


def header_codes(source: str) -> set:
    """The error codes the probe's leading `//!` comment names."""
    codes = set()
    for line in source.splitlines():
        if not line.startswith("//!"):
            break
        codes.update(CODE.findall(line))
    return codes


def verdict(name: str, source: str, compiled: bool, output: str):
    """`None` when the probe did what its name and header promise, otherwise why not."""
    if not name.endswith("_fails"):
        return None if compiled else "must compile and did not"
    if compiled:
        return "must not compile and did"
    named = header_codes(source)
    # rustc prints some errors with no code at all (`future cannot be sent between threads
    # safely`), so a probe whose output carries none is held to failing, not to a code.
    unnamed = set(ERROR_CODE.findall(output)) - named
    if unnamed:
        return f"printed {sorted(unnamed)}, which its header does not name; it names {sorted(named) or 'none'}"
    return None


def build(manifest: Path, target: Path, toolchain: str, name: str):
    command = ["cargo"]
    if toolchain:
        command.append(toolchain)
    command += ["build", "--locked", "--color", "never", "--manifest-path", str(manifest), "--target-dir", str(target), "--bin", name]
    done = subprocess.run(command, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, timeout=600)
    return done.returncode == 0, done.stdout


def run(crate: Path, toolchain: str, target: Path) -> int:
    manifest = crate / "Cargo.toml"
    sources = sorted((crate / "src" / "bin").glob("*.rs"))
    if not sources:
        print(f"no probe under {crate / 'src' / 'bin'}", file=sys.stderr)
        return 1
    failures = []
    for source in sources:
        compiled, output = build(manifest, target, toolchain, source.stem)
        why = verdict(source.stem, source.read_text(), compiled, output)
        if why:
            failures.append(source.stem)
            print(f"FAIL {source.stem}: {why}\n{output}")
    print(f"{len(sources) - len(failures)} of {len(sources)} probes held" + (f"; failed: {' '.join(failures)}" if failures else ""))
    return 1 if failures else 0


PLANTED = {
    # name: (source, what the verdict must be)
    "ok": ("fn main() {}\n", True),
    "broken": ("fn main() { let x: u32 = \"a\"; }\n", False),
    "wrong_fails": ("//! Expected: E0277.\nfn main() { let x: u32 = \"a\"; }\n", False),
    "right_fails": ("//! Expected: E0308.\nfn main() { let x: u32 = \"a\"; }\n", True),
    "unnamed_code_fails": ("fn main() { let x: u32 = \"a\"; }\n", False),
    "extra_code_fails": ("//! Expected: E0308.\nfn main() { let x: u32 = \"a\"; let _ = y; }\n", False),
    "codeless_fails": ("compile_error!(\"x\");\nfn main() {}\n", True),
    "compiles_fails": ("fn main() {}\n", False),
    "plain_error_fails": ("//! Expected: E0277.\ncompile_error!(\"x\");\nfn main() {}\n", True),
}


def self_test(toolchain: str) -> int:
    """Builds the planted probes in a crate of their own: each verdict must be the one expected,
    so a check that passed everything, or refused everything, is caught here."""
    with tempfile.TemporaryDirectory() as scratch:
        crate = Path(scratch) / "planted"
        (crate / "src" / "bin").mkdir(parents=True)
        (crate / "Cargo.toml").write_text('[package]\nname = "planted"\nversion = "0.0.0"\nedition = "2021"\n\n[workspace]\n')
        names = {}
        for name, (source, _) in PLANTED.items():
            (crate / "src" / "bin" / f"{name}.rs").write_text(source)
            names[name] = source
        # `--locked` needs a lockfile, and a probe crate with no dependency has nothing to lock.
        subprocess.run(["cargo", "generate-lockfile", "--manifest-path", str(crate / "Cargo.toml")], check=True, capture_output=True)
        bad = []
        for name, (source, held) in PLANTED.items():
            compiled, output = build(crate / "Cargo.toml", Path(scratch) / "target", toolchain, name)
            got = verdict(name, source, compiled, output) is None
            if got != held:
                bad.append(f"{name}: expected {'held' if held else 'a failure'}, got {'held' if got else 'a failure'}")
        for line in bad:
            print(f"SELF-TEST FAIL {line}")
        if not bad:
            print(f"self-test: {len(PLANTED)} planted probes got their expected verdicts")
        return 1 if bad else 0


def main(argv) -> int:
    toolchain = next((a for a in argv if a.startswith("+")), "")
    if "--self-test" in argv:
        return self_test(toolchain)
    target = Path(os.environ.get("PROBES_TARGET_DIR", ROOT / "target" / "probes"))
    return run(PROBES, toolchain, target)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
