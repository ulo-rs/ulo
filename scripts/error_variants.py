#!/usr/bin/env python3
"""Fails when a struct variant of a public error enum is not `#[non_exhaustive]` (DESIGN §10.2).

    python3 scripts/error_variants.py check nightly-2026-10-11
    python3 scripts/error_variants.py scan target/doc/ulo.json [more.json ..]
    python3 scripts/error_variants.py --self-test

The rule: a field added to a variant of a public error breaks no caller that destructures it, so
every struct variant of such an enum carries `#[non_exhaustive]`, and code outside the crate
writes `..` beside the fields it names and builds the variant through a constructor.

Which enums the rule covers: every public enum of the crate that implements `std::error::Error`,
and every public enum of the crate an error holds or hands out, followed transitively: the type of
a public field or a variant's payload of a covered enum or of a public error struct, and the
return type of a public inherent method of an error type, through `Box`, `Arc`, `Vec`, `Option`,
slices, references and any other generic argument. An enum reached only through another struct,
such as a value type an error's accessor returns, or through a method of an enum that is no error,
is not covered. `FailureReason`, held by `ConnectError`, is covered, and so is `WiringError`,
handed out by `WiringErrors::iter`; `ulo_rpc::Frame`, which the methods of `Codec` return, a
`Codec` being a field of `FrameUnencodable`, is not.

`check` builds the rustdoc JSON of every publishable library crate of the workspace with the
nightly toolchain given, into `CARGO_TARGET_DIR` or `target`, and scans it; `scan` reads JSON
files already built. Both run the self-test first: a planted unmarked variant must be found, a
marked one and an enum the walk does not reach must not.

Exit status 1 on a finding, 0 on none, 2 on a usage error. Standard library only.
"""

import json
import os
import pathlib
import subprocess
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
from public_reach import body, hidden, kind_of, types_in  # noqa: E402

ERROR_TRAIT = (["core", "error", "Error"], ["std", "error", "Error"])


class Crate:
    def __init__(self, doc):
        self.index = doc["index"]
        self.paths = doc["paths"]
        self.name = self.index[str(doc["root"])]["name"]

    def item(self, ident):
        item = self.index.get(str(ident))
        if item is None or item.get("crate_id") != 0 or hidden(item):
            return None
        return item

    def is_error(self, item):
        for impl in body(item).get("impls", []):
            inner = body(self.index.get(str(impl)) or {"inner": {"impl": {}}})
            trait = inner.get("trait") if isinstance(inner, dict) else None
            if trait and (self.paths.get(str(trait["id"])) or {}).get("path") in ERROR_TRAIT:
                return True
        return False

    def held(self, item):
        """The ids of every type `item`'s public fields and variant payloads name, and, for an
        error type, its public inherent methods' return types."""
        inner, found, ids = body(item), [], []
        shape = inner.get("kind")
        if isinstance(shape, dict) and "plain" in shape:
            ids = shape["plain"]["fields"]
        elif isinstance(shape, dict) and "tuple" in shape:
            ids = [i for i in shape["tuple"] if i is not None]
        for variant in inner.get("variants", []):
            vk = body(self.index[str(variant)])["kind"]
            if isinstance(vk, dict) and "tuple" in vk:
                ids += [i for i in vk["tuple"] if i is not None]
            elif isinstance(vk, dict) and "struct" in vk:
                ids += vk["struct"]["fields"]
        for ident in ids:
            field = self.index[str(ident)]
            if field.get("visibility") == "public" or inner.get("variants") is not None:
                found += types_in(body(field))
        for impl in inner.get("impls", []) if self.is_error(item) else []:
            impl = self.index.get(str(impl))
            if impl is None or hidden(impl) or body(impl).get("trait") or body(impl).get("blanket_impl") is not None:
                continue
            for member in body(impl).get("items", []):
                method = self.index[str(member)]
                if kind_of(method) == "function" and method.get("visibility") == "public" and not hidden(method):
                    output = body(method)["sig"].get("output")
                    found += types_in(output) if output else []
        return found

    def covered(self):
        """Every covered enum, by id."""
        queue = [ident for ident, item in self.index.items()
                 if item.get("crate_id") == 0 and not hidden(item) and kind_of(item) in ("enum", "struct") and self.is_error(item)]
        seen, enums = set(queue), set()
        while queue:
            ident = queue.pop()
            item = self.index[str(ident)]
            if kind_of(item) == "enum":
                enums.add(str(ident))
            for named in self.held(item):
                local = self.item(named)
                if local is None or str(named) in seen:
                    continue
                if kind_of(local) == "enum" or (kind_of(local) == "struct" and self.is_error(local)):
                    seen.add(str(named))
                    queue.append(str(named))
        return enums

    def findings(self):
        out = []
        for ident in sorted(self.covered(), key=int):
            item = self.index[ident]
            path = "::".join((self.paths.get(ident) or {}).get("path") or [self.name, item["name"]])
            for variant in body(item)["variants"]:
                v = self.index[str(variant)]
                shape = body(v)["kind"]
                if isinstance(shape, dict) and "struct" in shape and not any("non_exhaustive" in str(a) for a in v.get("attrs", [])):
                    out.append(f"{path}::{v['name']}")
        return out


def scan(files):
    found = []
    for file in files:
        found += Crate(json.loads(pathlib.Path(file).read_text(encoding="utf-8"))).findings()
    return found


def publishable_libraries(root):
    meta = json.loads(subprocess.run(
        ["cargo", "metadata", "--no-deps", "--format-version", "1", "--locked"],
        cwd=root, check=True, capture_output=True, text=True).stdout)
    out = []
    for package in meta["packages"]:
        if package.get("publish") == []:
            continue
        if any("lib" in target["kind"] for target in package["targets"]):
            out.append(package["name"])
    return sorted(out)


def check(root, toolchain, skip):
    target = pathlib.Path(os.environ.get("CARGO_TARGET_DIR") or root / "target")
    files = []
    for package in publishable_libraries(root):
        if package in skip:
            continue
        subprocess.run(
            ["cargo", f"+{toolchain}", "rustdoc", "-p", package, "--lib", "--locked", "--all-features", "--",
             "-Z", "unstable-options", "--output-format", "json"],
            cwd=root, check=True,
        )
        files.append(target / "doc" / f"{package.replace('-', '_')}.json")
    return report(scan(files))


def report(findings):
    for finding in findings:
        print(f"{finding}: a struct variant of a public error enum without `#[non_exhaustive]`")
    return 1 if findings else 0


def sample():
    """`Boom` implements `Error` and holds `Why` in a variant; `Value` is reached only through the
    struct `Holder`, which `Boom::holder` returns, and `Other` only through `Why::other`, a method
    of an enum that is no error. Each has one unmarked struct variant, and `Boom` a marked one
    too."""
    def enum(ident, name, variants, impls=()):
        return {"id": ident, "crate_id": 0, "name": name, "attrs": [], "visibility": "public",
                "inner": {"enum": {"variants": variants, "impls": list(impls)}}}

    def variant(ident, name, fields, marked=False):
        attrs = ["#[non_exhaustive]"] if marked else []
        return {"id": ident, "crate_id": 0, "name": name, "attrs": attrs, "visibility": "default",
                "inner": {"variant": {"kind": {"struct": {"fields": fields, "has_stripped_fields": False}}}}}

    def field(ident, ty):
        return {"id": ident, "crate_id": 0, "name": f"f{ident}", "attrs": [], "visibility": "default",
                "inner": {"struct_field": ty}}

    def path(ident):
        return {"resolved_path": {"path": "x", "id": ident, "args": None}}

    index = {
        "0": {"id": 0, "crate_id": 0, "name": "top", "attrs": [], "visibility": "public", "inner": {"module": {"items": [1, 20, 30, 40]}}},
        "1": enum(1, "Boom", [2, 3], impls=[10, 11]),
        "2": variant(2, "Open", [4]),
        "3": variant(3, "Closed", [5], marked=True),
        "4": field(4, path(20)),
        "5": field(5, {"primitive": "u32"}),
        "10": {"id": 10, "crate_id": 0, "name": None, "attrs": [], "visibility": "default",
               "inner": {"impl": {"trait": {"path": "Error", "id": 99, "args": None}, "items": [], "blanket_impl": None}}},
        "11": {"id": 11, "crate_id": 0, "name": None, "attrs": [], "visibility": "default",
               "inner": {"impl": {"trait": None, "items": [12], "blanket_impl": None}}},
        "12": {"id": 12, "crate_id": 0, "name": "holder", "attrs": [], "visibility": "public",
               "inner": {"function": {"sig": {"inputs": [], "output": path(30)}}}},
        "20": enum(20, "Why", [21], impls=[23]),
        "23": {"id": 23, "crate_id": 0, "name": None, "attrs": [], "visibility": "default",
               "inner": {"impl": {"trait": None, "items": [24], "blanket_impl": None}}},
        "24": {"id": 24, "crate_id": 0, "name": "other", "attrs": [], "visibility": "public",
               "inner": {"function": {"sig": {"inputs": [], "output": path(50)}}}},
        "50": enum(50, "Other", [51]),
        "51": variant(51, "Spare", [52]),
        "52": field(52, {"primitive": "u16"}),
        "21": variant(21, "Late", [22]),
        "22": field(22, {"primitive": "u64"}),
        "30": {"id": 30, "crate_id": 0, "name": "Holder", "attrs": [], "visibility": "public",
               "inner": {"struct": {"kind": {"plain": {"fields": [31], "has_stripped_fields": False}}, "impls": []}}},
        "31": {"id": 31, "crate_id": 0, "name": "value", "attrs": [], "visibility": "public", "inner": {"struct_field": path(40)}},
        "40": enum(40, "Value", [41]),
        "41": variant(41, "Pair", [42]),
        "42": field(42, {"primitive": "u8"}),
    }
    paths = {"1": {"crate_id": 0, "path": ["top", "Boom"], "kind": "enum"},
             "20": {"crate_id": 0, "path": ["top", "Why"], "kind": "enum"},
             "30": {"crate_id": 0, "path": ["top", "Holder"], "kind": "struct"},
             "40": {"crate_id": 0, "path": ["top", "Value"], "kind": "enum"},
             "50": {"crate_id": 0, "path": ["top", "Other"], "kind": "enum"},
             "99": {"crate_id": 1, "path": ["core", "error", "Error"], "kind": "trait"}}
    return {"root": 0, "index": index, "paths": paths, "external_crates": {"1": {"name": "core"}}}


def self_test():
    found = Crate(sample()).findings()
    if found != ["top::Boom::Open", "top::Why::Late"]:
        print(f"self-test: expected the unmarked variants of `Boom` and `Why` alone, found {found}", file=sys.stderr)
        return False
    return True


def main(argv):
    if not self_test():
        return 1
    if argv == ["--self-test"]:
        print("self-test passed")
        return 0
    if len(argv) >= 2 and argv[0] == "check":
        skip = set(argv[argv.index("--skip") + 1:]) if "--skip" in argv else set()
        return check(pathlib.Path(__file__).resolve().parent.parent, argv[1], skip)
    if len(argv) >= 2 and argv[0] == "scan":
        return report(scan(argv[1:]))
    print(__doc__, file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
