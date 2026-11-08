#!/usr/bin/env python3
"""Fails when this tree could not be released as `release-plz.toml` describes.

    python3 scripts/release_check.py check BASELINE [--skip PACKAGE ..]
    python3 scripts/release_check.py lockstep [ROOT]
    python3 scripts/release_check.py --self-test

BASELINE is a checkout of the last release, the tree the newest `v*` tag names. `check` runs
three steps against it and stops at the first that fails:

1. The lockstep rule, on this tree. The core is every `[[package]]` of `release-plz.toml` with
   `version_group = "core"`. It must hold every publishable crate that owns a doc-hidden module
   (`#[doc(hidden)]` on a `pub mod __x` or on a `pub use` of one), every proc-macro crate, every
   crate whose source writes an absolute `::ulo..::` path into a string literal, which is how a
   code generator such as `ulo-build` is recognised, and every crate a member depends on; and no
   other crate. Every member carries the version of `ulo`, the core's anchor, and while that
   version is a pre-release so does every leaf: the workspace releases at one version until the
   first release that is not a pre-release.
2. `release-plz update --registry-manifest-path BASELINE/Cargo.toml`, in a copy of this tree, since
   it rewrites manifests and refuses a dirty tree. Given that path it compares each crate with
   BASELINE's instead of with the crate published on crates.io. It must report on every publishable crate, and the
   versions it writes must keep the lockstep rule.
3. `cargo semver-checks` of every publishable library crate in both trees against BASELINE. A
   proc-macro or binary-only crate has no rustdoc API for it to compare, and it passes over one
   named with `-p` without a word, so those are named in the summary instead. The release
   type comes from the two versions: `minor` when Cargo's caret requirement on the baseline
   version accepts the current one, which includes an unchanged version and a pre-release step
   such as `0.1.0-alpha.2` to `0.1.0-alpha.3`; `major` otherwise. Checking equal pre-release
   versions with no release type skips every lint, and so does `--release-type major`, which
   permits every break. A major release is therefore checked as a minor one: its breaks are listed
   in the log and need no entry, since the version accepts them. Each crate must report a run of
   more than zero checks, and each break of a crate that is not a major release must be accepted
   in `semver-breaks.txt`. A crate BASELINE does not publish, one added since or made publishable
   since, has no release to compare with; the summary names each one.

`semver-breaks.txt` lists `break <package> <lint> <count>  # reason`: a break since the last
release that is accepted, with the number of distinct items the lint reports in that package. The
check fails on a break no entry accepts, on an entry whose count differs from the run's, and on an
entry nothing matches, so its entries go once BASELINE is a release that carries their breaks.

`--skip` leaves a package out of step 3, and out of release-plz's own semver check in step 2, on
a machine that cannot build its rustdoc: `ulo-rpc-kafka` with its `tls` feature needs OpenSSL
headers. CI passes none. `lockstep` runs step 1 alone. Both
commands run the self-test first, which plants violations of each rule and fails unless every one
is refused and the legal inputs pass.

Builds go to `CARGO_TARGET_DIR`, or `target/release-check` under ROOT when it is unset. Exit status
1 on a finding, 0 on none, 2 on a usage or list-file error. Standard library only, and Python 3.9.
"""

import json
import os
import pathlib
import re
import shutil
import subprocess
import sys
import tempfile

CONFIG = "release-plz.toml"
BREAKS = "semver-breaks.txt"
GROUP = "core"
ANCHOR = "ulo"

# Other attributes and doc comments may sit between `#[doc(hidden)]` and the item.
HIDDEN = re.compile(
    r"#\[doc\(hidden\)\]\s*(?:(?:#\[[^\]]*\]|//[^\n]*)\s*)*pub\s+(?:mod\s+__\w+|use\s+[^;]*\b__\w+\s*;)"
)
GENERATED_PATH = re.compile(r'"[^"\n]*(?<![\w:])::ulo(?:_\w+)?::')
ANSI = re.compile(r"\x1b\[[0-9;]*m")
FILE_REF = re.compile(r",?\s*(?:previously )?(?:in|at)(?: file)? \S+:\d+")


class UsageError(Exception):
    pass


def read_group(text):
    """The packages `release-plz.toml` puts in the core group, in file order."""
    names, current, in_package = [], {}, False

    def close():
        if in_package and current.get("version_group") == GROUP:
            if "name" not in current:
                raise UsageError(f"{CONFIG}: a [[package]] in group `{GROUP}` has no name")
            names.append(current["name"])

    for raw in text.splitlines():
        line = raw.split("#", 1)[0].strip()
        if not line:
            continue
        if line.startswith("["):
            close()
            in_package, current = line == "[[package]]", {}
            continue
        m = re.match(r'^([A-Za-z_]+)\s*=\s*"([^"]*)"$', line)
        if in_package and m:
            current[m.group(1)] = m.group(2)
    close()
    return names


def source_facts(src):
    """Whether a crate's source owns a doc-hidden module and whether it writes `::ulo` paths."""
    owner = generator = False
    for path in sorted(pathlib.Path(src).rglob("*.rs")):
        text = path.read_text(encoding="utf-8")
        owner = owner or bool(HIDDEN.search(text))
        code = "\n".join(l for l in text.splitlines() if not l.lstrip().startswith("//"))
        generator = generator or bool(GENERATED_PATH.search(code))
    return owner, generator


def packages(root):
    """`{name: {version, deps, owner, proc_macro, library, generator}}` for the publishable members."""
    out = subprocess.run(
        ["cargo", "metadata", "--no-deps", "--format-version", "1", "--offline",
         "--manifest-path", str(pathlib.Path(root) / "Cargo.toml")],
        capture_output=True, text=True,
    )
    if out.returncode != 0:
        raise UsageError(f"cargo metadata failed in {root}:\n{out.stderr}")
    meta = json.loads(out.stdout)
    members = {p["name"] for p in meta["packages"]}
    found = {}
    for p in meta["packages"]:
        if p["publish"] is not None:
            continue
        owner, generator = source_facts(pathlib.Path(p["manifest_path"]).parent / "src")
        found[p["name"]] = {
            "version": p["version"],
            "deps": sorted({d["name"] for d in p["dependencies"]
                            if d["name"] in members and d["kind"] in (None, "build")}),
            "owner": owner,
            "proc_macro": any("proc-macro" in t["kind"] for t in p["targets"]),
            "library": any(k in ("lib", "rlib") for t in p["targets"] for k in t["kind"]),
            "generator": generator,
        }
    return found


def lockstep_findings(pkgs, group):
    """What keeps `group` from being the core `pkgs` require, at one version."""
    found = []
    group = list(group)
    for name in sorted({n for n in group if group.count(n) > 1}):
        found.append(f"`{name}` is listed in group `{GROUP}` more than once")
    group = set(group)
    if ANCHOR not in group:
        found.append(f"`{ANCHOR}`, the core's anchor, is not in group `{GROUP}`")
    for name in sorted(group - set(pkgs)):
        found.append(f"`{name}` is in group `{GROUP}` but is no publishable member")
    why = {}
    for name, p in pkgs.items():
        if p["owner"]:
            why[name] = "owns a doc-hidden module"
        elif p["proc_macro"]:
            why[name] = "is a proc-macro crate"
        elif p["generator"]:
            why[name] = "writes `::ulo` paths into generated code"
    todo = sorted(why)
    while todo:
        name = todo.pop()
        for dep in pkgs[name]["deps"]:
            if dep in pkgs and dep not in why:
                why[dep] = f"is a dependency of `{name}`, in the core"
                todo.append(dep)
    for name in sorted(set(why) - group):
        found.append(f"`{name}` {why[name]}, so it belongs in group `{GROUP}`")
    for name in sorted((group & set(pkgs)) - set(why)):
        found.append(
            f"`{name}` is in group `{GROUP}` but owns no doc-hidden module, is no macro or "
            f"generator crate, and no core crate depends on it"
        )
    if ANCHOR in pkgs:
        anchor = pkgs[ANCHOR]["version"]
        for name in sorted(group & set(pkgs)):
            if pkgs[name]["version"] != anchor:
                found.append(f"`{name}` is at {pkgs[name]['version']}, the core at {anchor}")
        if "-" in anchor.split("+", 1)[0]:
            for name in sorted(set(pkgs) - group):
                if pkgs[name]["version"] != anchor:
                    found.append(f"`{name}` is at {pkgs[name]['version']}, and every crate is at "
                                 f"the core's {anchor} while it is a pre-release")
    return found


def without_semver_check(text, names):
    """`release-plz.toml` with release-plz's own semver check off for `names`."""
    for name in names:
        line = f'name = "{name}"'
        if line in text:
            text = text.replace(line, f"{line}\nsemver_check = false", 1)
        else:
            text += f'\n[[package]]\n{line}\nsemver_check = false\n'
    return text


def copy_tree(root, dest, skipped=()):
    """The working tree's tracked and unignored files, as one commit of a new repository, with
    release-plz's semver check off for the `skipped` packages."""
    listed = subprocess.run(
        ["git", "-C", str(root), "ls-files", "-z", "--cached", "--others", "--exclude-standard"],
        capture_output=True, check=True,
    ).stdout.decode().split("\0")
    for rel in filter(None, listed):
        src = pathlib.Path(root) / rel
        if src.is_file():
            target = dest / rel
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(src, target)
    if skipped:
        config = dest / CONFIG
        config.write_text(without_semver_check(config.read_text(), skipped))
    git = ["git", "-C", str(dest), "-c", "user.name=release-check", "-c", "user.email=release-check@invalid"]
    subprocess.run(git[:3] + ["init", "-q"], check=True)
    subprocess.run(git + ["add", "-A"], check=True)
    subprocess.run(git + ["commit", "-q", "--no-verify", "-m", "release check"], check=True)


def release_plz_findings(log, pkgs):
    """What a `release-plz update` log shows it skipped: a publishable crate it did not version."""
    seen = set(re.findall(r"determining next version for (\S+) \S+", ANSI.sub("", log)))
    if not seen:
        return ["release-plz determined no version: its run checked nothing"]
    return [f"release-plz did not version `{n}`" for n in sorted(set(pkgs) - seen)]


def parse_version(text):
    m = re.fullmatch(r"(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?(?:\+\S+)?", text)
    if not m:
        raise UsageError(f"not a version: {text}")
    pre = tuple((0, int(x), "") if x.isdigit() else (1, 0, x) for x in m.group(4).split(".")) if m.group(4) else None
    return int(m.group(1)), int(m.group(2)), int(m.group(3)), pre


def newer_or_equal(base, cur):
    if cur[:3] != base[:3]:
        return cur[:3] > base[:3]
    if base[3] is None or cur[3] is None:
        return cur[3] is None or base[3] is not None
    return cur[3] >= base[3]


def release_type(base, cur):
    """`minor` when Cargo's `^base` accepts `cur`, `major` when it does not, `None` for a step back."""
    b, c = parse_version(base), parse_version(cur)
    if not newer_or_equal(b, c):
        return None
    if c[3] is not None and c[:3] != b[:3]:
        return "major"  # a pre-release of another version is outside `^base`
    if b[0] > 0:
        same = c[0] == b[0]
    elif b[1] > 0:
        same = c[:2] == b[:2]
    else:
        same = c[:3] == b[:3]
    return "minor" if same else "major"


def parse_semver_log(log):
    """`{package: {checks, breaks: {lint: set(items)}}}` from a `cargo semver-checks` log."""
    found, name, lint, items = {}, None, None, False
    for raw in ANSI.sub("", log).splitlines():
        m = re.match(r"^\s+Checking (\S+) v\S+ -> v\S+ ", raw)
        if m:
            name, lint, items = m.group(1), None, False
            found[name] = {"checks": None, "breaks": {}}
            continue
        m = re.match(r"^\s+Checked \[[^\]]*\] (\d+) checks?:", raw)
        if m and name:
            found[name]["checks"] = int(m.group(1))
            continue
        m = re.match(r"^--- failure ([a-z0-9_]+):", raw)
        if m and name:
            lint, items = m.group(1), False
            found[name]["breaks"].setdefault(lint, set())
            continue
        if raw.startswith("Failed in:"):
            items = True
            continue
        if items and lint and raw.startswith("  ") and raw.strip():
            found[name]["breaks"][lint].add(FILE_REF.sub("", raw.strip()))
        elif items and not raw.startswith("  "):
            items = False
    return found


def read_breaks(text):
    """`{(package, lint): (count, reason)}` from `semver-breaks.txt`."""
    entries = {}
    for n, raw in enumerate(text.splitlines(), 1):
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        body, _, reason = line.partition(" # ")
        parts = body.split()
        if len(parts) != 4 or parts[0] != "break" or not parts[3].isdigit():
            raise UsageError(f"{BREAKS}:{n}: expected `break <package> <lint> <count>  # reason`")
        if not reason.strip():
            raise UsageError(f"{BREAKS}:{n}: an entry carries its reason after ` # `")
        key = (parts[1], parts[2])
        if key in entries:
            raise UsageError(f"{BREAKS}:{n}: `{parts[1]} {parts[2]}` is listed twice")
        entries[key] = (int(parts[3]), reason.strip())
    return entries


def semver_findings(report, expected, accepted, skipped=(), major=()):
    """What a parsed run leaves unchecked or unaccepted, and the entries it leaves unmatched. The
    breaks of a crate in `major` are accepted by its version, and an entry for one is stale."""
    found = []
    if not expected:
        found.append("no crate is in both trees, so nothing was checked")
    for name in sorted(expected):
        got = report.get(name)
        if got is None:
            found.append(f"`{name}` was not checked")
        elif not got["checks"]:
            found.append(f"`{name}` ran no checks: every lint was skipped")
    for name in sorted(set(report) - set(expected)):
        found.append(f"`{name}` was checked but no check was asked of it")
    seen = set()
    for name, got in sorted(report.items()):
        for lint, items in sorted(got["breaks"].items()):
            if name in major:
                continue
            seen.add((name, lint))
            entry = accepted.get((name, lint))
            listing = "".join(f"\n    {i}" for i in sorted(items))
            if entry is None:
                found.append(
                    f"`{name}` breaks `{lint}` and {BREAKS} does not accept it; to accept it, add\n"
                    f"  break {name} {lint} {len(items)}  # <why the break is wanted>{listing}"
                )
            elif entry[0] != len(items):
                found.append(
                    f"`{name}` breaks `{lint}` in {len(items)} items and {BREAKS} accepts "
                    f"{entry[0]}; the items now:{listing}"
                )
    for name, lint in sorted(set(accepted) - seen):
        if name in major:
            found.append(f"`{name}` is a major release, whose version accepts its breaks; "
                         f"remove `{name} {lint}` from {BREAKS}")
        elif name not in skipped:
            found.append(f"{BREAKS} accepts `{name} {lint}`, which this run did not report")
    return found


def semver_targets(base_pkgs, cur_pkgs, skipped=()):
    """`({release type: [crate]}, [crate with no library], [crate the release does not publish],
    findings)` for the publishable crates of the current tree.

    cargo-semver-checks reads a library's rustdoc and passes over a proc-macro or binary-only
    crate named with `-p` without a word, so those are listed apart rather than asked for. A crate
    the release does not publish has nothing to be compared with, and is listed apart too, so the
    summary names it rather than leaving it out of the count."""
    by_type, no_library, found = {}, [], []
    unreleased = sorted(set(cur_pkgs) - set(base_pkgs) - set(skipped))
    for name in sorted(set(base_pkgs) & set(cur_pkgs)):
        if name in skipped:
            continue
        if not cur_pkgs[name]["library"]:
            no_library.append(name)
            continue
        kind = release_type(base_pkgs[name]["version"], cur_pkgs[name]["version"])
        if kind is None:
            found.append(f"`{name}` is at {cur_pkgs[name]['version']}, behind the release's "
                         f"{base_pkgs[name]['version']}")
            continue
        by_type.setdefault(kind, []).append(name)
    return by_type, no_library, unreleased, found


def semver_command(root, baseline, kind, names):
    """`--release-type major` skips every lint, so a major release runs as a minor one and its
    breaks are listed rather than checked."""
    cmd = ["cargo", "semver-checks", "--manifest-path", str(pathlib.Path(root) / "Cargo.toml"),
           "--baseline-root", str(baseline), "--release-type", "minor" if kind == "major" else kind,
           "--color", "never"]
    for name in names:
        cmd += ["-p", name]
    return cmd


def run_semver(root, baseline, by_type, env):
    """Each release type's `cargo semver-checks` run, as one log."""
    log = []
    for kind, names in sorted(by_type.items()):
        cmd = semver_command(root, baseline, kind, names)
        print(f"semver: {len(names)} crate(s) as a {kind} release", flush=True)
        out = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, env=env)
        sys.stdout.write(out.stdout)
        log.append(out.stdout)
    return "\n".join(log)


def print_findings(step, found):
    for f in found:
        print(f"{step}: {f}", file=sys.stderr)
    return 1 if found else 0


def lockstep(root):
    pkgs = packages(root)
    group = read_group((root / CONFIG).read_text())
    if print_findings("lockstep", lockstep_findings(pkgs, group)):
        return 1, pkgs
    core = set(group)
    print(f"lockstep: {len(core)} core crates at {pkgs[ANCHOR]['version']}, "
          f"{len(pkgs) - len(core)} leaves")
    return 0, pkgs


def check(root, baseline, skipped):
    status, cur = lockstep(root)
    if status:
        return 1
    base = packages(baseline)
    for name in skipped:
        if name not in cur:
            raise UsageError(f"--skip names `{name}`, which is no publishable member")
    env = dict(os.environ)
    env.setdefault("CARGO_TARGET_DIR", str(root / "target" / "release-check"))
    env["NO_COLOR"] = "1"

    with tempfile.TemporaryDirectory(prefix="release-check-") as tmp:
        work = pathlib.Path(tmp) / "tree"
        copy_tree(root, work, skipped)
        out = subprocess.run(
            ["release-plz", "update", "--config", CONFIG,
             "--registry-manifest-path", str(baseline / "Cargo.toml")],
            cwd=work, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, env=env,
        )
        sys.stdout.write(out.stdout)
        if out.returncode != 0:
            print(f"release-plz: `update` exited {out.returncode}", file=sys.stderr)
            return 1
        found = release_plz_findings(out.stdout, cur)
        after = packages(work)
        found += [f"after the update, {f}" for f in lockstep_findings(after, read_group((work / CONFIG).read_text()))]
        if print_findings("release-plz", found):
            return 1
        print(f"release-plz: {len(cur)} crates versioned, the core at {after[ANCHOR]['version']}")

    accepted = read_breaks((root / BREAKS).read_text()) if (root / BREAKS).exists() else {}
    by_type, no_library, unreleased, found = semver_targets(base, cur, set(skipped))
    expected = {n for names in by_type.values() for n in names}
    report = parse_semver_log(run_semver(root, baseline, by_type, env))
    found += semver_findings(report, expected, accepted, set(skipped), set(by_type.get("major", ())))
    if print_findings("semver", found):
        return 1
    print(f"semver: {len(report)} of {len(expected)} library crates checked against "
          f"{base[ANCHOR]['version']}; no library API in {len(no_library)} ({', '.join(no_library)})"
          + (f"; not published at {base[ANCHOR]['version']}, so not compared: {', '.join(unreleased)}"
             if unreleased else "")
          + (f"; skipped {', '.join(skipped)}" if skipped else ""))
    return 0


def pkg(version="1.0.0", deps=(), owner=False, proc_macro=False, generator=False, library=True):
    return {"version": version, "deps": list(deps), "owner": owner, "proc_macro": proc_macro,
            "library": library and not proc_macro, "generator": generator}


def legal_tree():
    return {
        "ulo": pkg(deps=["ulo-macros"], owner=True),
        "ulo-macros": pkg(deps=["ulo-codegen"], proc_macro=True),
        "ulo-codegen": pkg(),
        "ulo-build": pkg(generator=True),
        "ulo-http": pkg(deps=["ulo", "ulo-net"], owner=True),
        "ulo-net": pkg(deps=["ulo"]),
        "ulo-db": pkg(version="3.1.0", deps=["ulo", "ulo-http"]),
    }


LEGAL_GROUP = ["ulo", "ulo-macros", "ulo-codegen", "ulo-build", "ulo-http", "ulo-net"]

SEMVER_LOG = """\
    Checking ulo v1.0.0 -> v1.0.0 (assume minor change)
     Checked [   0.010s] 202 checks: 200 pass, 2 fail, 58 skip

--- failure struct_pub_field_missing: pub struct's pub field removed or renamed ---

Description:
A publicly-visible struct has at least one public field that is no longer available.
        ref: https://doc.rust-lang.org/cargo/reference/semver.html#item-remove

Failed in:
  field a of struct Limits, previously in file /x/baseline/crates/ulo/src/lib.rs:12
  field b of struct Limits, previously in file /x/baseline/crates/ulo/src/lib.rs:13
  field b of struct Limits, previously in file /x/baseline/crates/ulo/src/lib.rs:13

     Summary semver requires new major version: 1 major and 0 minor checks failed
    Finished [   1.000s] ulo
    Checking ulo-db v3.1.0 -> v3.1.0 (assume minor change)
     Checked [   0.004s] 202 checks: 202 pass, 58 skip
     Summary no semver update required
    Finished [   1.000s] ulo-db
"""


def self_test():
    failures = []

    def refused(label, found):
        if not found:
            failures.append(f"planted {label} was not refused")

    def passed(label, found):
        if found:
            failures.append(f"legal {label} was refused: {found}")

    toml = (
        '[workspace]\nsemver_check = true\n\n[[package]]\nname = "ulo"\nversion_group = "core"\n'
        'git_tag_name = "v{{ version }}"\n\n[[package]]\nname = "ulo-db"\ngit_tag_enable = false\n\n'
        '[[package]]\nversion_group = "core"  # trailing comment\nname = "ulo-http"\n'
    )
    if read_group(toml) != ["ulo", "ulo-http"]:
        failures.append(f"group reader read {read_group(toml)}")

    passed("tree", lockstep_findings(legal_tree(), LEGAL_GROUP))
    planted = {
        "owner outside the group": [n for n in LEGAL_GROUP if n != "ulo-http"],
        "proc-macro crate outside the group": [n for n in LEGAL_GROUP if n != "ulo-macros"],
        "generator outside the group": [n for n in LEGAL_GROUP if n != "ulo-build"],
        "dependency of the core outside the group": [n for n in LEGAL_GROUP if n != "ulo-net"],
        "leaf inside the group": LEGAL_GROUP + ["ulo-db"],
        "member that is no package": LEGAL_GROUP + ["ulo-gone"],
        "group without its anchor": [n for n in LEGAL_GROUP if n != "ulo"],
        "member listed twice": LEGAL_GROUP + ["ulo-net"],
    }
    for label, group in planted.items():
        refused(label, lockstep_findings(legal_tree(), group))
    skewed = legal_tree()
    skewed["ulo-net"]["version"] = "1.0.1"
    refused("core member at another version", lockstep_findings(skewed, LEGAL_GROUP))
    alpha = {n: dict(p, version="1.0.0-alpha.1") for n, p in legal_tree().items()}
    passed("pre-release tree at one version", lockstep_findings(alpha, LEGAL_GROUP))
    alpha["ulo-db"]["version"] = "1.0.0-alpha.2"
    refused("leaf at another version during a pre-release", lockstep_findings(alpha, LEGAL_GROUP))

    owner, generator = False, False
    with tempfile.TemporaryDirectory() as tmp:
        src = pathlib.Path(tmp)
        (src / "a.rs").write_text('#[doc(hidden)]\npub mod __private;\n')
        (src / "b.rs").write_text('let s = "impl ::ulo_grpc::Method for X {}";\n')
        owner, generator = source_facts(src)
        if not (owner and generator):
            failures.append(f"source facts read owner={owner} generator={generator} on planted source")
        for f in src.iterdir():
            f.unlink()
        (src / "c.rs").write_text(
            '#[doc(hidden)]\npub use ulo_transport::__derive;\n'
        )
        if not source_facts(src)[0]:
            failures.append("a doc-hidden re-export of a `__` module was not read as owning one")
        (src / "c.rs").write_text(
            '#[doc(hidden)]\n#[cfg(feature = "x")]\n/// Glue.\npub mod __glue {}\n'
        )
        if not source_facts(src)[0]:
            failures.append("a doc-hidden module behind another attribute was not read as owned")
        (src / "c.rs").write_text(
            'pub mod private;\n// "::ulo_http::x" in a comment\nlet s = "ulo_http::Path";\n'
        )
        if source_facts(src) != (False, False):
            failures.append(f"legal source read as {source_facts(src)}")

    types = {
        ("0.1.0-alpha.2", "0.1.0-alpha.2"): "minor",
        ("0.1.0-alpha.2", "0.1.0-alpha.3"): "minor",
        ("0.1.0-alpha.2", "0.1.0"): "minor",
        ("0.1.0", "0.1.1"): "minor",
        ("0.1.0", "0.2.0"): "major",
        ("0.1.0", "0.2.0-alpha.1"): "major",
        ("0.0.3", "0.0.4"): "major",
        ("1.2.0", "1.3.0"): "minor",
        ("1.2.0", "2.0.0"): "major",
        ("0.1.0-alpha.3", "0.1.0-alpha.2"): None,
        ("0.1.0", "0.1.0-alpha.9"): None,
        ("1.2.0", "1.1.9"): None,
    }
    for (base, cur), want in types.items():
        got = release_type(base, cur)
        if got != want:
            failures.append(f"release type of {base} -> {cur} read as {got}, not {want}")

    base = legal_tree()
    cur = dict(legal_tree(), **{"ulo-cli": pkg(library=False), "ulo-new": pkg()})
    cur["ulo-db"] = pkg(version="4.0.0")
    cur["ulo-net"] = pkg(version="0.9.0")
    by_type, no_library, unreleased, found = semver_targets(base, cur, skipped={"ulo-build"})
    if by_type != {"minor": ["ulo", "ulo-codegen", "ulo-http"], "major": ["ulo-db"]} \
            or no_library != ["ulo-macros"] or len(found) != 1:
        failures.append(f"semver targets read as {by_type}, {no_library}, {found}")
    # `ulo-cli` and `ulo-new` are publishable here and absent from the release; every other crate
    # is in both trees and is checked or listed apart above.
    if unreleased != ["ulo-cli", "ulo-new"]:
        failures.append(f"crates the release does not publish read as {unreleased}, not ulo-cli and ulo-new")
    if semver_targets(cur, cur)[2]:
        failures.append("a crate the release publishes was read as one it does not")

    report = parse_semver_log(SEMVER_LOG)
    want = {"ulo": {"checks": 202, "breaks": {"struct_pub_field_missing": {
        "field a of struct Limits", "field b of struct Limits"}}},
        "ulo-db": {"checks": 202, "breaks": {}}}
    if report != want:
        failures.append(f"semver log read as {report}")
    accepted = {("ulo", "struct_pub_field_missing"): (2, "reason")}
    passed("semver run", semver_findings(report, {"ulo", "ulo-db"}, accepted))
    refused("unaccepted break", semver_findings(report, {"ulo", "ulo-db"}, {}))
    refused("break count that differs", semver_findings(
        report, {"ulo", "ulo-db"}, {("ulo", "struct_pub_field_missing"): (1, "reason")}))
    refused("entry nothing matches", semver_findings(
        report, {"ulo", "ulo-db"}, {**accepted, ("ulo-db", "function_missing"): (1, "r")}))
    passed("entry of a skipped crate", semver_findings(
        report, {"ulo", "ulo-db"}, {**accepted, ("ulo-x", "function_missing"): (1, "r")},
        skipped={"ulo-x"}))
    refused("crate that was not checked", semver_findings(report, {"ulo", "ulo-db", "ulo-net"}, accepted))
    refused("run of zero crates", semver_findings({}, set(), {}))
    if "minor" not in semver_command("/r", "/b", "major", ["ulo-db"]):
        failures.append("a major release was run with a release type that skips every lint")
    passed("break of a major release", semver_findings(report, {"ulo", "ulo-db"}, {}, major={"ulo"}))
    refused("entry for a major release", semver_findings(report, {"ulo", "ulo-db"}, accepted, major={"ulo"}))
    vacuous = parse_semver_log(SEMVER_LOG.replace("202 checks: 202 pass, 58 skip", "0 checks: 0 pass, 260 skip"))
    refused("crate whose lints were all skipped", semver_findings(vacuous, {"ulo", "ulo-db"}, accepted))

    if read_breaks("# c\n\nbreak ulo function_missing 2  # wanted\n") != {("ulo", "function_missing"): (2, "wanted")}:
        failures.append("break list reader misread a legal entry")
    for bad in ("break ulo function_missing 2\n", "break ulo function_missing two  # r\n",
                "allow ulo function_missing 2  # r\n",
                "break ulo f 1  # r\nbreak ulo f 1  # r\n"):
        try:
            read_breaks(bad)
            failures.append(f"break list reader accepted {bad!r}")
        except UsageError:
            pass

    skip_toml = '[[package]]\nname = "a"\nversion_group = "core"\n'
    if read_group(without_semver_check(skip_toml, ["a", "b"])) != ["a"] or \
            without_semver_check(skip_toml, ["a", "b"]).count("semver_check = false") != 2:
        failures.append("the semver-check opt-out for a skipped package was not written")

    rp_log = "\x1b[32m INFO\x1b[0m determining next version for ulo 1.0.0\n  determining next version for ulo-db 3.1.0\n"
    passed("release-plz log", release_plz_findings(rp_log, {"ulo": 0, "ulo-db": 0}))
    refused("release-plz log missing a crate", release_plz_findings(rp_log, {"ulo": 0, "ulo-db": 0, "ulo-net": 0}))
    refused("release-plz log of no crate", release_plz_findings("", {"ulo": 0}))

    for f in failures:
        print(f"self-test: {f}", file=sys.stderr)
    return not failures


def main(argv):
    # The tools' logs and the findings, which go to stderr, interleave in the order they happen.
    sys.stdout.reconfigure(line_buffering=True)
    if argv == ["--self-test"]:
        if not self_test():
            return 1
        print("self-test passed")
        return 0
    root = pathlib.Path(__file__).resolve().parent.parent
    try:
        if argv[:1] == ["lockstep"] and len(argv) <= 2:
            if not self_test():
                return 1
            return lockstep(pathlib.Path(argv[1]).resolve() if len(argv) == 2 else root)[0]
        if argv[:1] == ["check"] and len(argv) >= 2:
            rest, skipped = argv[2:], []
            while rest:
                if rest[0] != "--skip" or len(rest) < 2:
                    raise UsageError(__doc__)
                skipped.append(rest[1])
                rest = rest[2:]
            if not self_test():
                return 1
            return check(root, pathlib.Path(argv[1]).resolve(), skipped)
    except UsageError as e:
        print(e, file=sys.stderr)
        return 2
    print(__doc__, file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
