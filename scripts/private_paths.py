"""Fails when a crate's source names another crate's `__private` or `__derive` module.

A crate's `__private` (and `__derive`) module is the ABI between that crate and the code its macros
generate: doc-hidden, outside semver, reshaped in any release. Code that is not generated reaches
another crate through its public API; what a link, host or module author shares with the
first-party crates is public (`ulo_transport::sync`, `ulo_rpc::link`, `ulo::secrets`). This check
reads every package's `.rs` source under `crates/` and `examples/` and reports each path that names
`__private` or `__derive` and is not the package's own:

- a path whose head is `crate`, `self`, `super` or `$crate` (the last inside the package's own
  `macro_rules!`), the package's own library name, or a module the package declares, is the
  package's own, provided the package declares or re-exports a module of that name; a package
  that declares none owns none, and every such path in it is reported;
- any other head, another crate's name or an alias of one (`use ulo_rpc as r;` then
  `r::__private`), is reported, as is a bare `__private::x` in a package that declares none
  (a glob import of a crate root brings the module into scope);
- a `use` tree is expanded first, so `use a::{b, __private::x}`, `use a::__private::x as y` and
  `use a::{__private}` are read as the full paths they import.

Not scanned: a package whose manifest says `proc-macro = true`, and each package listed as
`emitter` in `private-paths.txt`: both write these paths into the code they generate, which is the
module's purpose. Comments, string and character literals are skipped, as is doc text outside a
code block; a fenced block in a `///`, `//!`, `/** */` or `/*! */` comment that rustdoc compiles
(no tag, `rust`, `no_run`, `should_panic`, an `edition` tag) is scanned as code, and one marked
`ignore`, `text`, `compile_fail` or another language is not.

Limits: a README pulled in with `#[doc = include_str!(..)]`, a `#[doc = "..."]` attribute and a
doc block indented four spaces rather than fenced are not read; macro expansion is not run, so a
path a non-proc-macro crate's `macro_rules!` builds from fragments (`$krate::__private` with
`$krate` bound at the call) is reported only with the head it is written with; names are not
resolved per scope, so in a package that has a `__private` of its own, a bare `__private::x`
under a glob import of another crate in a submodule, or a head aliasing another crate under the
name of one of the package's modules, is taken as the package's own; and a module the package
declares that glob-re-exports another crate is taken as the package's own.

`private-paths.txt` lists `emitter <package>` and `allow <package> <path>` entries, each with its
reason after a `#`. `<path>` is the reported path up to the item after `__private`, or the module
itself for a re-export of it: `ulo_transport::__private::Param`, `ulo_transport::__derive`. The
check fails on a finding no entry allows, on an entry no finding matches, and on an `emitter` that
is not a package, is a proc-macro crate or names no such path, so the list holds no stale entry;
a repeated entry is an allow-file error:

    python3 scripts/private_paths.py check [ROOT]
    python3 scripts/private_paths.py scan DIR [--allow FILE]
    python3 scripts/private_paths.py --self-test

`check` reads `ROOT/crates` and `ROOT/examples` (default: the repository root) against
`ROOT/private-paths.txt`; `scan` reads every package under `DIR`, against no entries unless
`--allow` names a file. Both run the self-test first, which plants violations and legal uses in a
temporary tree and asserts this script's own exit codes on them. Exit status 1 on a finding or a
stale entry, 0 on none, 2 on a usage or allow-file error. Standard library only.
"""

import os
import pathlib
import re
import subprocess
import sys
import tempfile

HIDDEN = ("__private", "__derive")
OWN_HEADS = {"crate", "self", "super", "$crate"}
ALLOW_FILE = "private-paths.txt"
COMPILED_TAGS = {"", "rust", "no_run", "should_panic"}



def manifest(path):
    """`(package name, library name, is proc-macro)` of a manifest, or `None` for a workspace root."""
    section, name, lib, proc_macro = None, None, None, False
    for raw in path.read_text(encoding="utf-8").splitlines():
        line = raw.split("#", 1)[0].strip()
        m = re.match(r"^\[([^\]]+)\]$", line)
        if m:
            section = m.group(1).strip()
            continue
        m = re.match(r'^([A-Za-z_-]+)\s*=\s*(.+)$', line)
        if not m:
            continue
        key, value = m.group(1), m.group(2).strip()
        if section == "package" and key == "name":
            name = value.strip('"')
        elif section == "lib" and key == "name":
            lib = value.strip('"')
        elif section == "lib" and key in ("proc-macro", "proc_macro"):
            proc_macro = value == "true"
    if name is None:
        return None
    return name, lib or name.replace("-", "_"), proc_macro


def walk(top):
    """Every file under `top`, build output and VCS directories pruned."""
    for base, dirs, names in os.walk(top):
        dirs[:] = sorted(d for d in dirs if d != ".git" and d != "target" and not d.startswith("target-"))
        for name in sorted(names):
            yield pathlib.Path(base, name)


def packages(top):
    """Every package under `top`: `{dir: (name, lib, proc_macro)}`."""
    found = {}
    for path in walk(top):
        if path.name == "Cargo.toml":
            info = manifest(path)
            if info:
                found[path.parent] = info
    return found


def sources(top, owned):
    """`{package dir: [.rs files]}`, each file under the package directory nearest to it."""
    dirs = sorted(owned, key=lambda d: len(d.parts), reverse=True)
    files = {d: [] for d in owned}
    for path in walk(top):
        if path.suffix != ".rs":
            continue
        for d in dirs:
            if d in path.parents:
                files[d].append(path)
                break
    return files



IDENT = re.compile(r"[A-Za-z_][A-Za-z0-9_]*")
RAW = re.compile(r'(?:b|c)?r(#*)"')
NUMBER = re.compile(r"[0-9][0-9A-Za-z_.]*")


def doc_blocks(doc_lines):
    """The code of each compiled fenced block in one run of doc-comment lines: `[(line, text)]`."""
    code, inside, compiled = [], None, False
    for number, text in doc_lines:
        m = re.match(r"^\s*(`{3,}|~{3,})\s*(.*)$", text)
        if m and inside is None:
            inside = m.group(1)
            tags = {t for t in re.split(r"[,\s]+", m.group(2).strip()) if t}
            tags = {t for t in tags if not t.startswith("edition")}
            compiled = tags <= COMPILED_TAGS
            continue
        if m and inside is not None and m.group(1).startswith(inside[0]) and len(m.group(1)) >= len(inside):
            inside = None
            continue
        if inside is not None and compiled:
            stripped = text.strip()
            if stripped == "#":
                continue
            if stripped.startswith("# "):
                text = stripped[2:]
            code.append((number, text))
    return code


def tokens(text):
    """`[(kind, value, line)]`: `ident` (raw identifiers unprefixed, `$name` kept with its `$`),
    `::`, and single punctuation. Comments, literals and lifetimes produce nothing; the code of each
    compiled doc block is lexed in place of its comment lines."""
    out = []
    i, n, line = 0, len(text), 1
    doc_run = []

    def flush_docs():
        if doc_run:
            for number, code in doc_blocks(doc_run):
                out.extend((k, v, number) for k, v, _ in tokens(code))
            doc_run.clear()

    while i < n:
        c = text[i]
        if c == "\n":
            line += 1
            i += 1
            continue
        if c.isspace():
            i += 1
            continue
        if text.startswith("//", i):
            end = text.find("\n", i)
            end = n if end == -1 else end
            body = text[i + 2:end]
            if body.startswith("/") and not body.startswith("//") or body.startswith("!"):
                doc_run.append((line, body[1:]))
            else:
                flush_docs()
            i = end
            continue
        flush_docs()
        if text.startswith("/*", i):
            start, first = i, line
            depth, i = 1, i + 2
            while i < n and depth:
                if text.startswith("/*", i):
                    depth, i = depth + 1, i + 2
                elif text.startswith("*/", i):
                    depth, i = depth - 1, i + 2
                else:
                    line += text[i] == "\n"
                    i += 1
            # `/** */` and `/*! */` are doc comments, whose compiled blocks rustdoc runs as the
            # line forms' are; `/**/` and `/***` are not.
            body = text[start + 2:i - 2]
            if body.startswith("!") or (body.startswith("*") and not body.startswith("**")):
                rows = body[1:].split("\n")
                starred = all(re.match(r"^\s*\*", r) for r in rows[1:] if r.strip())
                if starred:
                    rows = [rows[0]] + [re.sub(r"^\s*\* ?", "", r) for r in rows[1:]]
                for number, code in doc_blocks([(first + k, r) for k, r in enumerate(rows)]):
                    out.extend((kind, value, number) for kind, value, _ in tokens(code))
            continue
        m = RAW.match(text, i)
        if m:
            close = '"' + m.group(1)
            end = text.find(close, m.end())
            end = n if end == -1 else end + len(close)
            line += text.count("\n", i, end)
            i = end
            continue
        if c == '"' or (c in "bc" and text.startswith('"', i + 1)):
            i += 1 if c == '"' else 2
            while i < n and text[i] != '"':
                if text[i] == "\\":
                    i += 1
                line += text[i] == "\n" if i < n else 0
                i += 1
            i += 1
            continue
        if c == "'" or (c == "b" and text.startswith("'", i + 1)):
            j = i + (2 if c == "b" else 1)
            if j < n and text[j] == "\\":
                end = text.find("'", j + 2)
                i = n if end == -1 else end + 1
                continue
            if j + 1 < n and text[j + 1] == "'":
                i = j + 2
                continue
            m = IDENT.match(text, j)
            i = m.end() if m else j
            continue
        if text.startswith("r#", i) and IDENT.match(text, i + 2):
            m = IDENT.match(text, i + 2)
            out.append(("ident", m.group(0), line))
            i = m.end()
            continue
        if c == "$" and IDENT.match(text, i + 1):
            m = IDENT.match(text, i + 1)
            out.append(("ident", "$" + m.group(0), line))
            i = m.end()
            continue
        m = IDENT.match(text, i)
        if m:
            out.append(("ident", m.group(0), line))
            i = m.end()
            continue
        if c.isdigit():
            i = NUMBER.match(text, i).end()
            continue
        if text.startswith("::", i):
            out.append(("::", "::", line))
            i += 2
            continue
        out.append(("punct", c, line))
        i += 1
    flush_docs()
    return out



def use_tree(toks, i, prefix):
    """Parses one use tree at `toks[i]`: `(paths, next index)`, each path a list of segments with
    its line, `*` for a glob, and `as` aliases recorded as the alias."""
    segs, paths = list(prefix), []
    line = toks[i][2] if i < len(toks) else 0
    if i < len(toks) and toks[i][0] == "::":
        i += 1
    while i < len(toks):
        kind, value, line = toks[i]
        if kind == "ident" and value != "as":
            segs.append(value)
            i += 1
            if i < len(toks) and toks[i][0] == "::":
                i += 1
                continue
            break
        if kind == "punct" and value == "*":
            segs.append("*")
            i += 1
            break
        if kind == "punct" and value == "{":
            i += 1
            while i < len(toks) and not (toks[i][0] == "punct" and toks[i][1] == "}"):
                sub, after = use_tree(toks, i, segs)
                paths.extend(sub)
                # A token no tree starts with, an attribute's `#[..]` on an item of the group,
                # is stepped over.
                i = after if after > i else i + 1
                if i < len(toks) and toks[i][0] == "punct" and toks[i][1] == ",":
                    i += 1
            return paths, i + 1
        break
    alias = None
    if i + 1 < len(toks) and toks[i][1] == "as" and toks[i + 1][0] == "ident":
        alias = toks[i + 1][1]
        i += 2
    if segs and segs[-1] == "self":
        segs = segs[:-1]
    if segs:
        paths.append((segs, line, alias))
    return paths, i


def paths_in(toks):
    """Every path naming `__private` or `__derive`: `[(segments, line, alias, is_use)]`, and the
    module names the source declares (`mod x`)."""
    found, modules, consumed = [], set(), set()
    i = 0
    while i < len(toks):
        kind, value, line = toks[i]
        if kind == "ident" and value == "mod" and i + 1 < len(toks) and toks[i + 1][0] == "ident":
            modules.add(toks[i + 1][1])
        if kind == "ident" and value == "use" and i + 1 < len(toks) and toks[i + 1][1] != "<":
            j, trees = i + 1, []
            sub, j = use_tree(toks, j, [])
            trees.extend(sub)
            for segs, at, alias in trees:
                if any(s in HIDDEN for s in segs) or alias in HIDDEN:
                    found.append((segs, at, alias, True))
            consumed.update(range(i, j))
            i = j
            continue
        i += 1
    for i, (kind, value, line) in enumerate(toks):
        if i in consumed or kind != "ident" or value not in HIDDEN:
            continue
        before = toks[i - 1] if i else None
        if before and before[0] == "punct" and before[1] == ".":
            continue
        if before and before[0] == "ident" and before[1] == "mod":
            continue
        start = i
        while start >= 2 and toks[start - 1][0] == "::" and toks[start - 2][0] == "ident":
            start -= 2
        # An unqualified name with no `::` after it is a field, binding or function of that name,
        # not a path into a module.
        if start == i and not (i + 1 < len(toks) and toks[i + 1][0] == "::"):
            continue
        segs = [toks[k][1] for k in range(start, i + 1, 2)]
        if i + 2 < len(toks) and toks[i + 1][0] == "::" and toks[i + 2][0] == "ident":
            segs.append(toks[i + 2][1])
        found.append((segs, line, None, False))
    return found, modules


def key_of(segs):
    """The reported path: up to the item after the hidden module, or the module itself."""
    at = next(k for k, s in enumerate(segs) if s in HIDDEN)
    return "::".join(segs[: at + 2])


def scan_package(directory, info, files):
    """`[(file, line, key)]` for one package's violations."""
    name, lib, _ = info
    per_file, modules, owns = [], set(), set()
    for path in files:
        found, mods = paths_in(tokens(path.read_text(encoding="utf-8", errors="replace")))
        modules |= mods
        per_file.append((path, found))
        for segs, _, alias, is_use in found:
            if is_use and (alias in HIDDEN or (alias is None and segs[-1] in HIDDEN)):
                owns.add(alias or segs[-1])
    owns |= {m for m in modules if m in HIDDEN}
    legal_heads = OWN_HEADS | {lib} | modules
    out = []
    for path, found in per_file:
        for segs, line, alias, is_use in found:
            if not any(s in HIDDEN for s in segs):
                continue
            at = next(k for k, s in enumerate(segs) if s in HIDDEN)
            hidden = segs[at]
            head = segs[0]
            own = hidden in owns and (at == 0 or (head in legal_heads and all(s in legal_heads for s in segs[1:at])))
            if not own:
                out.append((path, line, key_of(segs)))
    return out



def read_allow(path):
    emitters, allowed = {}, {}
    if path is None:
        return emitters, allowed
    for number, raw in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        text = raw.split("#", 1)[0].split()
        if not text:
            continue
        reason = raw.split("#", 1)[1].strip() if "#" in raw else ""
        if not reason:
            raise ValueError(f"{path.name}:{number}: an entry carries its reason after a `#`")
        if text[0] == "emitter" and len(text) == 2:
            if text[1] in emitters:
                raise ValueError(f"{path.name}:{number}: repeats line {emitters[text[1]]}")
            emitters[text[1]] = number
        elif text[0] == "allow" and len(text) == 3:
            if (text[1], text[2]) in allowed:
                raise ValueError(f"{path.name}:{number}: repeats line {allowed[(text[1], text[2])]}")
            allowed[(text[1], text[2])] = number
        else:
            raise ValueError(f"{path.name}:{number}: neither `emitter <package>` nor `allow <package> <path>`")
    return emitters, allowed


def run(tops, allow_path, label):
    try:
        emitters, allowed = read_allow(allow_path)
    except (OSError, ValueError) as err:
        print(err, file=sys.stderr)
        return 2
    owned, files = {}, {}
    for top in tops:
        if top.is_dir():
            found = packages(top)
            owned.update(found)
            files.update(sources(top, found))
    names = {info[0]: d for d, info in owned.items()}
    source = allow_path.name if allow_path else ALLOW_FILE
    findings, matched, problems = [], set(), []
    emitter_hits, proc_macros = {}, set()
    for directory, info in sorted(owned.items()):
        hits = scan_package(directory, info, files.get(directory, []))
        if info[2] or info[0] in emitters:
            emitter_hits[info[0]] = len(hits)
            if info[2]:
                proc_macros.add(info[0])
            continue
        for path, line, key in hits:
            entry = (info[0], key)
            if entry in allowed:
                matched.add(entry)
            else:
                findings.append(f"{path.relative_to(label)}:{line}: `{info[0]}` names `{key}`, another crate's hidden module")
    for package, number in sorted(emitters.items(), key=lambda kv: kv[1]):
        if package not in names:
            problems.append(f"{source}:{number}: `{package}` is no package under the scanned trees; delete the line")
        elif package in proc_macros:
            problems.append(f"{source}:{number}: `{package}` is a proc-macro crate, which is not scanned; delete the line")
        elif emitter_hits.get(package, 0) == 0:
            problems.append(f"{source}:{number}: `{package}` names no hidden path; delete the line")
    for entry, number in sorted(allowed.items(), key=lambda kv: kv[1]):
        if entry not in matched:
            problems.append(f"{source}:{number}: allows `{entry[0]}` naming `{entry[1]}`, which the scan no longer finds; delete the line")
    for line in findings + problems:
        print(line)
    return 1 if findings or problems else 0



PLANTED_BAD = {
    "plain.rs": "use ulo_rpc::__private::ClientProbe;\n",
    "renamed.rs": "use ulo_rpc::__private::HoldUntil as Hold;\n",
    "nested.rs": "use ulo_rpc::{link, __private::{CANCEL_HOLD}};\n",
    "module.rs": "pub use ulo_transport::{__derive};\n",
    "expr.rs": "fn f() { let _ = ::ulo::__private::setting_url(\"x\"); }\n",
    "alias.rs": "use ulo_rpc as r;\nfn g() { let _ = r::__private::CANCEL_HOLD; }\n",
    "bare.rs": "use ulo::*;\nfn h() { __private::setting_url(\"x\"); }\n",
    "doc.rs": "/// ```\n/// # use ulo::__private::hidden_in_a_doctest;\n/// ```\nfn d() {}\n",
    "qualified.rs": "fn q<T>() -> <T as ulo::__private::Probe>::Out { todo!() }\n",
    "own_name.rs": "fn o() { crate::__private::x(); }\n",
    "block_doc.rs": "/**\n * ```\n * ulo::__private::in_a_block_doc();\n * ```\n */\nfn b() {}\n",
}

PLANTED_GOOD = {
    "comments.rs": "// ulo::__private::x\n/* ulo::__private::y /* nested ulo::__private */ still */\n//! ulo::__private::z\n",
    "strings.rs": "const A: &str = \"ulo::__private::x\";\nconst B: &str = r#\"ulo::__private::\"y\"#;\nconst C: char = '\"'; const D: &[u8] = b\"ulo::__private\";\n",
    "lifetimes.rs": "fn l<'a>(x: &'a str) -> &'a str { x }\n",
    "doc_text.rs": "/// Names `ulo::__private::x` in prose.\n///\n/// ```text\n/// ulo::__private::y\n/// ```\n///\n/// ```ignore\n/// ulo::__private::z\n/// ```\nfn t() {}\n",
    "fields.rs": "fn f(s: S) { let _ = s.__private; }\n",
    "own_items.rs": "struct P { __private: () }\nfn __private() -> P { let __private = (); P { __private } }\n",
}

OWNER_FILES = {
    "src/lib.rs": "#[doc(hidden)]\npub mod __private;\npub mod inner;\nmacro_rules! m { () => { $crate::__private::x() }; }\nfn a() { crate::__private::x(); self::__private::x(); inner::__private::x(); }\n",
    "src/__private.rs": "pub fn x() {}\n",
    "src/inner.rs": "pub mod __private { pub fn x() { super::super::__private::x(); } }\n",
    "tests/own.rs": "use owner::__private::x;\n",
}


def write_tree(root, crates):
    for name, (manifest_text, files) in crates.items():
        base = root / "crates" / name
        for rel, text in files.items():
            path = base / rel
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(text, encoding="utf-8")
        (base / "Cargo.toml").write_text(manifest_text, encoding="utf-8")


def package_toml(name, proc_macro=False):
    lib = "\n[lib]\nproc-macro = true\n" if proc_macro else ""
    return f'[package]\nname = "{name}"\nversion = "0.0.0"\n{lib}'


def invoke(args):
    return subprocess.run([sys.executable, "-I", str(pathlib.Path(__file__).resolve()), *args], capture_output=True, text=True)


def self_test():
    failures = []
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        bad = {f"src/{k}": v for k, v in PLANTED_BAD.items()}
        bad["src/lib.rs"] = "".join(f"mod {k[:-3]};\n" for k in PLANTED_BAD)
        write_tree(root, {
            "user": (package_toml("user"), bad),
            "clean": (package_toml("clean"), {"src/lib.rs": "".join(PLANTED_GOOD.values())}),
            "owner": (package_toml("owner"), OWNER_FILES),
            "emits": (package_toml("emits", proc_macro=True), {"src/lib.rs": "quote! { ::ulo::__private::x }\n"}),
            "codegen": (package_toml("codegen"), {"src/lib.rs": "quote! { ::ulo::__private::y }\n"}),
        })
        info = manifest(root / "crates" / "user" / "Cargo.toml")
        hits = scan_package(root / "crates" / "user", info, sorted((root / "crates" / "user" / "src").glob("*.rs")))
        reported = {(p.name, k) for p, _, k in hits}
        expected = {
            ("plain.rs", "ulo_rpc::__private::ClientProbe"),
            ("renamed.rs", "ulo_rpc::__private::HoldUntil"),
            ("nested.rs", "ulo_rpc::__private::CANCEL_HOLD"),
            ("module.rs", "ulo_transport::__derive"),
            ("expr.rs", "ulo::__private::setting_url"),
            ("alias.rs", "r::__private::CANCEL_HOLD"),
            ("bare.rs", "__private::setting_url"),
            ("doc.rs", "ulo::__private::hidden_in_a_doctest"),
            ("qualified.rs", "ulo::__private::Probe"),
            ("own_name.rs", "crate::__private::x"),
            ("block_doc.rs", "ulo::__private::in_a_block_doc"),
        }
        if reported != expected:
            failures.append(f"planted violations: missed {sorted(expected - reported)}, extra {sorted(reported - expected)}")
        for package in ("clean", "owner"):
            directory = root / "crates" / package
            files = sorted(directory.rglob("*.rs"))
            hits = scan_package(directory, manifest(directory / "Cargo.toml"), files)
            if hits:
                failures.append(f"planted legal uses in `{package}` reported: {[(p.name, k) for p, _, k in hits]}")

        allow = root / ALLOW_FILE
        entries = "".join(f"allow user {k}  # planted\n" for _, k in sorted(expected)) + "emitter codegen  # planted\n"
        allow.write_text(entries, encoding="utf-8")
        cases = [
            ("every finding allowed", entries, 0),
            ("one finding not allowed", entries.replace("allow user ulo_rpc::__private::ClientProbe  # planted\n", ""), 1),
            ("a stale allowance", entries + "allow user ulo::__private::gone  # planted\n", 1),
            ("a stale emitter", entries + "emitter clean  # planted\n", 1),
            ("an emitter that is no package", entries + "emitter nowhere  # planted\n", 1),
            ("the emitter not listed", entries.replace("emitter codegen  # planted\n", ""), 1),
            ("an entry without a reason", entries + "allow user ulo::__private::x\n", 2),
            ("a repeated entry", entries + "emitter codegen  # planted\n", 2),
            ("a proc-macro crate as an emitter", entries + "emitter emits  # planted\n", 1),
        ]
        for label, text, want in cases:
            allow.write_text(text, encoding="utf-8")
            got = invoke(["check", str(root), "--no-self-test"]).returncode
            if got != want:
                failures.append(f"`check` on {label}: exit {got}, expected {want}")
        clean = invoke(["scan", str(root / "crates" / "clean"), "--no-self-test"]).returncode
        dirty = invoke(["scan", str(root / "crates" / "user"), "--no-self-test"]).returncode
        if (clean, dirty) != (0, 1):
            failures.append(f"`scan`: exit {clean} on the clean package and {dirty} on the violating one, expected 0 and 1")
    for failure in failures:
        print(f"self-test: {failure}", file=sys.stderr)
    return not failures


def main(argv):
    skip = "--no-self-test" in argv
    argv = [a for a in argv if a != "--no-self-test"]
    if argv == ["--self-test"]:
        if not self_test():
            return 1
        print("self-test passed")
        return 0
    if not skip and not self_test():
        return 1
    if argv and argv[0] == "check" and len(argv) <= 2:
        root = pathlib.Path(argv[1]).resolve() if len(argv) == 2 else pathlib.Path(__file__).resolve().parent.parent
        allow = root / ALLOW_FILE
        return run([root / "crates", root / "examples"], allow if allow.exists() else None, root)
    if len(argv) in (2, 4) and argv[0] == "scan":
        top = pathlib.Path(argv[1]).resolve()
        allow = None
        if len(argv) == 4:
            if argv[2] != "--allow":
                print(__doc__, file=sys.stderr)
                return 2
            allow = pathlib.Path(argv[3]).resolve()
        return run([top], allow, top)
    print(__doc__, file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
