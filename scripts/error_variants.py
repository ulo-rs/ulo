#!/usr/bin/env python3
"""Fails when a struct variant of a public error enum is not `#[non_exhaustive]` (DESIGN §10.2).

    python3 scripts/error_variants.py check nightly-2026-10-11
    python3 scripts/error_variants.py scan target/doc/ulo.json [more.json ..]
    python3 scripts/error_variants.py --self-test

The rule: a field added to a variant of a public error breaks no caller that destructures it, so
every struct variant of such an enum carries `#[non_exhaustive]`, and code outside the crate
writes `..` beside the fields it names and builds the variant through a constructor.

Which enums the rule covers: every public enum of the crate an error holds or hands out, reached
by any path from a public type that implements `std::error::Error`: the type of a public field or
of a variant's payload, and the argument and return types of a public inherent method, of the
error and of every type reached from it, through `Box`, `Arc`, `Vec`, `Option`, slices,
references, type aliases and any other generic argument, into the other crates scanned beside
it. `FailureReason`, held by `ConnectError`, is covered; so are `WiringError`, handed out by
`WiringErrors::iter`, an enum held by a value struct an error's field names, an enum a reached
type's method returns or takes, and an enum of one crate that an error of another holds.

`check` builds the rustdoc JSON of every publishable library crate of the workspace with the
nightly toolchain given, into `CARGO_TARGET_DIR` or `target`, and scans it; `scan` reads JSON
files already built. Both run the self-test first: a planted unmarked variant must be found on
each path of the walk, and a marked one and one held only by a private field must not.

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
        self.crates = {str(k): v["name"] for k, v in doc.get("external_crates", {}).items()}
        self.outside = set()

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
        """The ids of every type `item`'s public fields and variant payloads name, and its public
        inherent methods' argument and return types."""
        inner, found, ids = body(item), [], []
        if kind_of(item) == "type_alias":
            return types_in(inner["type"])
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
        for impl in inner.get("impls", []):
            impl = self.index.get(str(impl))
            if impl is None or hidden(impl) or body(impl).get("trait") or body(impl).get("blanket_impl") is not None:
                continue
            for member in body(impl).get("items", []):
                method = self.index[str(member)]
                if kind_of(method) == "function" and method.get("visibility") == "public" and not hidden(method):
                    sig = body(method)["sig"]
                    found += [i for _, ty in sig.get("inputs", []) for i in types_in(ty)]
                    found += types_in(sig["output"]) if sig.get("output") else []
        return found

    def covered(self, seeds=frozenset()):
        """Every covered enum, by id: reached from this crate's errors, or from `seeds`, the
        `(crate, path)` of types other crates' errors reach. What this crate's walk reaches in
        another crate is left in `outside`."""
        queue = [ident for ident, item in self.index.items()
                 if item.get("crate_id") == 0 and not hidden(item) and kind_of(item) in ("enum", "struct") and self.is_error(item)]
        queue += [ident for ident, place in self.paths.items()
                  if place.get("crate_id") == 0 and (self.name, tuple(place["path"])) in seeds and self.item(ident) is not None]
        seen, enums = set(queue), set()
        while queue:
            ident = queue.pop()
            item = self.index[str(ident)]
            if kind_of(item) == "enum":
                enums.add(str(ident))
            for named in self.held(item):
                local = self.item(named)
                if local is None:
                    place = self.paths.get(str(named)) or {}
                    if place.get("crate_id", 0) != 0 and place.get("kind") in ("enum", "struct", "type_alias"):
                        self.outside.add((self.crates.get(str(place["crate_id"])), tuple(place["path"])))
                    continue
                if str(named) in seen:
                    continue
                if kind_of(local) in ("enum", "struct", "type_alias"):
                    seen.add(str(named))
                    queue.append(str(named))
        return enums

    def findings(self, seeds=frozenset()):
        out = []
        for ident in sorted(self.covered(seeds), key=int):
            item = self.index[ident]
            path = "::".join((self.paths.get(ident) or {}).get("path") or [self.name, item["name"]])
            for variant in body(item)["variants"]:
                v = self.index[str(variant)]
                shape = body(v)["kind"]
                if isinstance(shape, dict) and "struct" in shape and not any("non_exhaustive" in str(a) for a in v.get("attrs", [])):
                    out.append(f"{path}::{v['name']}")
        return out


def scan(files):
    return scan_docs([json.loads(pathlib.Path(file).read_text(encoding="utf-8")) for file in files])


def scan_docs(docs):
    """The findings of every crate, each walk seeded with what the others' walks reach in it,
    until no walk reaches anything new."""
    crates, seeds = [Crate(doc) for doc in docs], set()
    while True:
        for crate in crates:
            crate.covered(seeds)
        reached = set().union(*(crate.outside for crate in crates))
        if reached <= seeds:
            return [finding for crate in crates for finding in crate.findings(seeds)]
        seeds |= reached


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
    public field of the struct `Holder`, which `Boom::holder` returns, `Other` only through
    `Why::other`, a method of an enum that is no error, and `Arg` only as the argument of
    `Boom::feed`, inside an `Option`, `Aliased` only through the alias `Alias` in `Boom::Closed`,
    `far::Far` only through `Boom::Closed` in the other crate `far`, and `Loose` only through a
    private field of `Holder`, which the walk does not follow. Each has one unmarked struct
    variant, and `Boom` a marked one too."""
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
        "0": {"id": 0, "crate_id": 0, "name": "top", "attrs": [], "visibility": "public", "inner": {"module": {"items": [1, 20, 30, 40, 60, 70, 80, 81]}}},
        "1": enum(1, "Boom", [2, 3], impls=[10, 11]),
        "2": variant(2, "Open", [4]),
        "3": variant(3, "Closed", [5, 6, 7], marked=True),
        "4": field(4, path(20)),
        "5": field(5, {"primitive": "u32"}),
        "6": field(6, path(80)),
        "7": field(7, path(97)),
        "80": {"id": 80, "crate_id": 0, "name": "Alias", "attrs": [], "visibility": "public",
               "inner": {"type_alias": {"type": path(81), "generics": {"params": [], "where_predicates": []}}}},
        "81": enum(81, "Aliased", [82]),
        "82": variant(82, "Bare", [83]),
        "83": field(83, {"primitive": "i16"}),
        "10": {"id": 10, "crate_id": 0, "name": None, "attrs": [], "visibility": "default",
               "inner": {"impl": {"trait": {"path": "Error", "id": 99, "args": None}, "items": [], "blanket_impl": None}}},
        "11": {"id": 11, "crate_id": 0, "name": None, "attrs": [], "visibility": "default",
               "inner": {"impl": {"trait": None, "items": [12, 13], "blanket_impl": None}}},
        "13": {"id": 13, "crate_id": 0, "name": "feed", "attrs": [], "visibility": "public",
               "inner": {"function": {"sig": {"inputs": [["arg", {"resolved_path": {"path": "Option", "id": 98, "args": {"angle_bracketed": {"args": [{"type": path(60)}], "constraints": []}}}}]], "output": None}}}},
        "60": enum(60, "Arg", [61]),
        "61": variant(61, "Flag", [62]),
        "62": field(62, {"primitive": "bool"}),
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
               "inner": {"struct": {"kind": {"plain": {"fields": [31, 32], "has_stripped_fields": False}}, "impls": []}}},
        "31": {"id": 31, "crate_id": 0, "name": "value", "attrs": [], "visibility": "public", "inner": {"struct_field": path(40)}},
        "32": field(32, path(70)),
        "40": enum(40, "Value", [41]),
        "41": variant(41, "Pair", [42]),
        "42": field(42, {"primitive": "u8"}),
        "70": enum(70, "Loose", [71]),
        "71": variant(71, "Kept", [72]),
        "72": field(72, {"primitive": "i8"}),
    }
    paths = {"1": {"crate_id": 0, "path": ["top", "Boom"], "kind": "enum"},
             "20": {"crate_id": 0, "path": ["top", "Why"], "kind": "enum"},
             "30": {"crate_id": 0, "path": ["top", "Holder"], "kind": "struct"},
             "40": {"crate_id": 0, "path": ["top", "Value"], "kind": "enum"},
             "50": {"crate_id": 0, "path": ["top", "Other"], "kind": "enum"},
             "60": {"crate_id": 0, "path": ["top", "Arg"], "kind": "enum"},
             "70": {"crate_id": 0, "path": ["top", "Loose"], "kind": "enum"},
             "80": {"crate_id": 0, "path": ["top", "Alias"], "kind": "type_alias"},
             "81": {"crate_id": 0, "path": ["top", "Aliased"], "kind": "enum"},
             "97": {"crate_id": 2, "path": ["far", "Far"], "kind": "enum"},
             "98": {"crate_id": 1, "path": ["core", "option", "Option"], "kind": "enum"},
             "99": {"crate_id": 1, "path": ["core", "error", "Error"], "kind": "trait"}}
    return {"root": 0, "index": index, "paths": paths, "external_crates": {"1": {"name": "core"}, "2": {"name": "far"}}}


def sample_far():
    """The crate `far`: `Far`, with one unmarked struct variant, which no error of `far` holds."""
    return {"root": 0, "external_crates": {},
            "index": {"0": {"id": 0, "crate_id": 0, "name": "far", "attrs": [], "visibility": "public", "inner": {"module": {"items": [1]}}},
                      "1": {"id": 1, "crate_id": 0, "name": "Far", "attrs": [], "visibility": "public", "inner": {"enum": {"variants": [2], "impls": []}}},
                      "2": {"id": 2, "crate_id": 0, "name": "Spread", "attrs": [], "visibility": "default",
                            "inner": {"variant": {"kind": {"struct": {"fields": [3], "has_stripped_fields": False}}}}},
                      "3": {"id": 3, "crate_id": 0, "name": "f3", "attrs": [], "visibility": "default", "inner": {"struct_field": {"primitive": "u8"}}}},
            "paths": {"1": {"crate_id": 0, "path": ["far", "Far"], "kind": "enum"}}}


def self_test():
    found = scan_docs([sample(), sample_far()])
    want = ["far::Far::Spread", "top::Aliased::Bare", "top::Arg::Flag", "top::Boom::Open", "top::Other::Spare",
            "top::Value::Pair", "top::Why::Late"]
    if sorted(found) != want:
        print(f"self-test: expected {want}, found {sorted(found)}", file=sys.stderr)
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
