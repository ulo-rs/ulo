"""Fails when a crate's public API names a type the crate does not make reachable.

The rule (DESIGN §0.7): every type a crate's public signatures name is reachable from that crate,
through a public path or a re-export, so a user never adds a dependency no documentation names to
write a type a handler takes or returns. Types of `std`, `core`, `alloc` and `ulo` are exempt:
every application depends on `ulo` directly.

`public-reach.txt` lists the crates the rule is checked on (`crate <package>`) and the findings
accepted as they stand (`allow <crate> <item> <type>`, each with its reason after a `#`). The
check builds each crate's rustdoc JSON with the nightly toolchain given, scans it, and fails on a
finding the file does not allow and on an allowance no finding matches, so the list cannot hold a
stale entry:

    python3 scripts/public_reach.py check nightly-2026-10-11
    python3 scripts/public_reach.py scan target/doc/ulo_http.json [more.json ..]
    python3 scripts/public_reach.py --self-test

What it reads as a signature: function and method parameters and return types, public struct
fields, enum variant payloads, type aliases, constants and statics, the traits a public type
implements (blanket and auto-trait impls aside), and the traits named by an `impl Trait` or a
`dyn Trait` inside those types. Generic bounds are not read: a `T: Serialize` asks the user to
implement a trait, which the derive that implements it already brings. An item under
`#[doc(hidden)]` is no public API, and a re-export inside one makes nothing reachable. The
signatures of an item re-exported from another crate are that crate's and are not read here.

Exit status 1 on a finding, 0 on none. Standard library only.
"""

import json
import pathlib
import subprocess
import sys
import tempfile

# `ulo`: every application depends on it. serde: its traits arrive with the derive that
# implements them, which needs a serde dependency of the user's own.
EXEMPT = {"std", "core", "alloc", "ulo", "serde", "serde_core"}


def hidden(item):
    return any("hidden" in attr and "doc" in attr for attr in map(str, item.get("attrs", [])))


def kind_of(item):
    inner = item["inner"]
    return next(iter(inner)) if isinstance(inner, dict) else inner


def body(item):
    inner = item["inner"]
    return inner[kind_of(item)] if isinstance(inner, dict) else None


class Crate:
    def __init__(self, doc):
        self.doc = doc
        self.index = doc["index"]
        self.paths = doc["paths"]
        self.crates = {int(k): v["name"] for k, v in doc["external_crates"].items()}
        self.name = self.index[str(doc["root"])]["name"]
        self.reachable = set()
        self.prefixes = []
        self.items = []
        self.walk_module(str(doc["root"]), [self.name])

    def path_of(self, ident):
        entry = self.paths.get(str(ident))
        return entry["path"] if entry else None

    def walk_module(self, ident, at):
        module = self.index[ident]
        for child in body(module)["items"]:
            item = self.index.get(str(child))
            if item is None or hidden(item):
                continue
            kind = kind_of(item)
            if kind == "use":
                self.walk_use(body(item), at)
            elif kind == "module":
                self.walk_module(str(child), at + [item["name"]])
            else:
                self.reachable.add(child)
                self.items.append((child, at + [item["name"] or "?"]))

    def walk_use(self, use, at):
        target = use.get("id")
        if target is None:
            return
        local = self.index.get(str(target))
        if local is not None and local.get("crate_id") == 0:
            if kind_of(local) == "module":
                self.walk_module(str(target), at + [use["name"]])
            else:
                self.reachable.add(target)
                self.items.append((target, at + [use["name"]]))
            return
        self.reachable.add(target)
        entry = self.paths.get(str(target))
        if use.get("is_glob") or (entry and entry["kind"] in ("module", "crate")):
            if entry:
                self.prefixes.append(entry["path"])

    def reaches(self, ident):
        if ident in self.reachable:
            return True
        entry = self.paths.get(str(ident))
        if entry is None:
            return True
        if entry["crate_id"] != 0 and self.crates.get(entry["crate_id"]) in EXEMPT:
            return True
        return any(entry["path"][: len(p)] == p for p in self.prefixes)

    def findings(self):
        seen = set()
        out = []
        for ident, where in self.items:
            for named in self.named_by(self.index[str(ident)]):
                if not self.reaches(named) and (named, tuple(where)) not in seen:
                    seen.add((named, tuple(where)))
                    out.append(("::".join(self.path_of(named) or ["?"]), "::".join(where)))
        return out

    def named_by(self, item):
        kind, inner = kind_of(item), body(item)
        found = []
        if kind == "function":
            sig = inner["sig"]
            for _, ty in sig["inputs"]:
                found += types_in(ty)
            if sig.get("output"):
                found += types_in(sig["output"])
        elif kind in ("struct", "enum", "union"):
            found += self.fields(inner)
            for impl in inner.get("impls", []):
                found += self.impl_names(self.index.get(str(impl)))
        elif kind == "trait":
            for member in inner["items"]:
                found += self.named_by(self.index[str(member)])
        elif kind == "type_alias":
            found += types_in(inner["type"])
        elif kind in ("constant", "static"):
            found += types_in(inner.get("type"))
        elif kind == "assoc_const":
            found += types_in(inner.get("type"))
        elif kind == "assoc_type":
            if inner.get("type"):
                found += types_in(inner["type"])
        return found

    def fields(self, inner):
        found = []
        shape = inner.get("kind")
        ids = []
        if isinstance(shape, dict):
            if "plain" in shape:
                ids = shape["plain"]["fields"]
            elif "tuple" in shape:
                ids = [i for i in shape["tuple"] if i is not None]
        for variant in inner.get("variants", []):
            v = body(self.index[str(variant)])
            vk = v["kind"]
            if isinstance(vk, dict) and "tuple" in vk:
                ids += [i for i in vk["tuple"] if i is not None]
            elif isinstance(vk, dict) and "struct" in vk:
                ids += vk["struct"]["fields"]
        for ident in ids:
            field = self.index[str(ident)]
            if field.get("visibility") == "public" or inner.get("variants") is not None:
                found += types_in(body(field))
        return found

    def impl_names(self, impl):
        if impl is None or hidden(impl):
            return []
        inner = body(impl)
        if inner.get("is_synthetic") or inner.get("blanket_impl") is not None:
            return []
        found = []
        if inner.get("trait"):
            found.append(inner["trait"]["id"])
            found += args_in(inner["trait"].get("args"))
            return found
        for member in inner["items"]:
            item = self.index[str(member)]
            if item.get("visibility") == "public" and not hidden(item):
                found += self.named_by(item)
        return found


def args_in(args):
    found = []
    if not args:
        return found
    angle = args.get("angle_bracketed")
    if angle:
        for arg in angle.get("args", []):
            if isinstance(arg, dict) and "type" in arg:
                found += types_in(arg["type"])
        for constraint in angle.get("constraints", []):
            binding = constraint.get("binding", {})
            if isinstance(binding, dict) and "equality" in binding:
                eq = binding["equality"]
                if isinstance(eq, dict) and "type" in eq:
                    found += types_in(eq["type"])
    parenthesized = args.get("parenthesized")
    if parenthesized:
        for ty in parenthesized.get("inputs", []):
            found += types_in(ty)
        if parenthesized.get("output"):
            found += types_in(parenthesized["output"])
    return found


def bounds_in(bounds):
    found = []
    for bound in bounds or []:
        trait = bound.get("trait_bound", {}).get("trait") if isinstance(bound, dict) else None
        if trait:
            found.append(trait["id"])
            found += args_in(trait.get("args"))
    return found


def types_in(ty):
    if not isinstance(ty, dict):
        return []
    (kind, inner), = ty.items()
    if kind == "resolved_path":
        return [inner["id"]] + args_in(inner.get("args"))
    if kind in ("borrowed_ref", "raw_pointer"):
        return types_in(inner["type"])
    if kind in ("slice",):
        return types_in(inner)
    if kind == "array":
        return types_in(inner["type"])
    if kind == "tuple":
        return [i for t in inner for i in types_in(t)]
    if kind == "impl_trait":
        return bounds_in(inner)
    if kind == "dyn_trait":
        found = []
        for poly in inner.get("traits", []):
            found.append(poly["trait"]["id"])
            found += args_in(poly["trait"].get("args"))
        return found
    if kind == "qualified_path":
        return types_in(inner["self_type"]) + ([inner["trait"]["id"]] if inner.get("trait") else [])
    if kind == "function_pointer":
        sig = inner["sig"]
        found = [i for _, t in sig["inputs"] for i in types_in(t)]
        return found + (types_in(sig["output"]) if sig.get("output") else [])
    return []


def scan(files):
    """Every `(crate, item, type)` finding in the rustdoc JSON `files`."""
    found = []
    for file in files:
        crate = Crate(json.loads(pathlib.Path(file).read_text(encoding="utf-8")))
        found += [(crate.name, where, named) for named, where in crate.findings()]
    return found


def report(findings):
    for crate, where, named in findings:
        print(f"{crate}: `{where}` names `{named}`, which `{crate}` does not re-export")


def config(root):
    crates, allowed = [], {}
    for number, line in enumerate((root / "public-reach.txt").read_text(encoding="utf-8").splitlines(), 1):
        text = line.split("#", 1)[0].split()
        if not text:
            continue
        if text[0] == "crate" and len(text) == 2:
            crates.append(text[1])
        elif text[0] == "allow" and len(text) == 4 and "#" in line and line.split("#", 1)[1].strip():
            allowed[tuple(text[1:])] = number
        else:
            sys.exit(f"public-reach.txt:{number}: neither `crate <package>` nor `allow <crate> <item> <type>  # reason`")
    return crates, allowed


def check(root, toolchain):
    crates, allowed = config(root)
    files = []
    for package in crates:
        subprocess.run(
            ["cargo", f"+{toolchain}", "rustdoc", "-p", package, "--lib", "--locked", "--",
             "-Z", "unstable-options", "--output-format", "json"],
            cwd=root, check=True,
        )
        files.append(root / "target" / "doc" / f"{package.replace('-', '_')}.json")
    findings = scan(files)
    unexpected = [f for f in findings if f not in allowed]
    stale = sorted(number for key, number in allowed.items() if key not in findings)
    report(unexpected)
    for number in stale:
        print(f"public-reach.txt:{number}: allows a finding the scan no longer makes; delete the line")
    return 1 if unexpected or stale else 0


def sample(reexport):
    """A two-crate rustdoc document: `dep::Thing` named by `top::make`, re-exported or not."""
    index = {
        "0": {"id": 0, "crate_id": 0, "name": "top", "attrs": [], "visibility": "public",
              "inner": {"module": {"items": [1, 2] if reexport else [1]}}},
        "1": {"id": 1, "crate_id": 0, "name": "make", "attrs": [], "visibility": "public",
              "inner": {"function": {"sig": {"inputs": [], "output": {"resolved_path": {"path": "dep::Thing", "id": 9, "args": None}}}}}},
        "2": {"id": 2, "crate_id": 0, "name": "Thing", "attrs": [], "visibility": "public",
              "inner": {"use": {"source": "dep::Thing", "name": "Thing", "id": 9, "is_glob": False}}},
    }
    paths = {"9": {"crate_id": 1, "path": ["dep", "Thing"], "kind": "struct"}}
    return {"root": 0, "index": index, "paths": paths, "external_crates": {"1": {"name": "dep"}}}


def self_test():
    with tempfile.TemporaryDirectory() as tmp:
        missing, present = pathlib.Path(tmp, "missing.json"), pathlib.Path(tmp, "present.json")
        missing.write_text(json.dumps(sample(False)), encoding="utf-8")
        present.write_text(json.dumps(sample(True)), encoding="utf-8")
        flagged = Crate(json.loads(missing.read_text())).findings()
        clean = Crate(json.loads(present.read_text())).findings()
    if flagged != [("dep::Thing", "top::make")] or clean:
        print(f"self-test: unexported {flagged}, re-exported {clean}", file=sys.stderr)
        return False
    return True


def main(argv):
    if not self_test():
        return 1
    if argv == ["--self-test"]:
        print("self-test passed")
        return 0
    if len(argv) == 2 and argv[0] == "check":
        return check(pathlib.Path(__file__).resolve().parent.parent, argv[1])
    if len(argv) >= 2 and argv[0] == "scan":
        findings = scan(argv[1:])
        report(findings)
        return 1 if findings else 0
    print(__doc__, file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
