#!/usr/bin/env python3
"""Publishes the workspace's crates to crates.io, in dependency order, riding out 429s.

    python3 scripts/publish.py plan                    # order, new and existing names, wall time; publishes nothing
    python3 scripts/publish.py publish --dry-run       # `cargo publish --workspace --dry-run` over what is left
    python3 scripts/publish.py publish                 # publishes
    python3 scripts/publish.py --self-test

The publishable crates and their order come from `cargo metadata`: a crate is ready once every
publishable workspace crate it depends on is on crates.io at its version, through a normal or build
dependency or a dev-dependency that names a version (Cargo strips only a dev-dependency without
one from the published manifest, and resolves the rest against the registry). Among the ready
crates a name crates.io already has goes before a brand-new name,
because a new name draws on the stricter limit. Whether a name or a version exists is read from the
sparse index, which is a read-only query.

`publish` first requires a clean tree and a HEAD that the core tag `v<version>` names, the tag
RELEASING.md has the release commit carry; `--dry-run` requires the clean tree only. Each crate is
published with `cargo publish -p NAME --locked`. A crate already on crates.io at its version is
skipped, and so is one cargo reports as already uploaded, which is what a rerun after an
interruption meets. A 429 names a time ("try again after <date> GMT"); the script sleeps until
then plus 30 seconds and retries the same crate. Any other failure stops the run with cargo's
output. Everything published so far stays published, and running the command again resumes.

Tags are created locally and never pushed. Each leaf crate, one outside the `core` group of
`release-plz.toml`, gets an annotated tag `<crate>-v<version>` at HEAD once it is on crates.io. A
crate an earlier run uploaded is tagged only when the `.cargo_vcs_info.json` of its published
`.crate` names HEAD; otherwise the run says which commit it names and leaves it untagged. The core
tag is the release's own and is only checked. A tag that exists at another commit stops the run.

`--dry-run` runs one `cargo publish --workspace --dry-run --locked`, excluding the crates already on
crates.io at their version: Cargo packages the rest together and resolves their `=` pins on each
other locally, so every crate still to publish is verified before the first upload.

Standard library only, Python 3.9. Run it from the workspace root, or pass `--root DIR`.
"""

import datetime
import email.utils
import io
import json
import os
import pathlib
import re
import subprocess
import sys
import tarfile
import time
import urllib.error
import urllib.request

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import release_check  # noqa: E402

UA = "ulo-release-script (https://github.com/ulo-rs/ulo)"
INDEX = "https://index.crates.io"
DOWNLOAD = "https://static.crates.io/crates"
MARGIN = 30
# https://crates.io/docs/rate-limits, per account: a brand-new name draws on a burst of 5 and then
# one per 10 minutes; a new version of an existing name on a burst of 30 and then one per minute.
NEW_BURST, NEW_MINUTES = 5, 10
EXISTING_BURST, EXISTING_MINUTES = 30, 1
RETRY = re.compile(r"try again after ([A-Za-z]{3}, \d{1,2} [A-Za-z]{3} \d{4} \d{2}:\d{2}:\d{2} GMT)")
# Cargo's own check against the index, and crates.io's answer when the index lags behind it.
ALREADY = re.compile(r"crate \S+@\S+ already exists on |crate version `[^`]+` is already uploaded")


class Stop(Exception):
    """A condition the run refuses to continue past; the message is the whole report."""


def run(args, cwd):
    return subprocess.run(args, cwd=cwd, capture_output=True, text=True)


def http_get(url):
    req = urllib.request.Request(url, headers={"User-Agent": UA})
    try:
        with urllib.request.urlopen(req, timeout=30) as r:
            return r.status, r.read().decode("utf-8")
    except urllib.error.HTTPError as e:
        return e.code, ""


def sleep(seconds):
    time.sleep(seconds)


def now():
    return datetime.datetime.now(datetime.timezone.utc)


def retry_after(text):
    """The moment a 429 answer says to try again, as an aware datetime, or None."""
    m = RETRY.search(text)
    return email.utils.parsedate_to_datetime(m.group(1)) if m else None


def wait_seconds(when, at):
    return max(0.0, (when - at).total_seconds()) + MARGIN


def is_429(text):
    return "429" in text and "Too Many Requests" in text


def index_path(name):
    n = name.lower()
    if len(n) <= 2:
        return f"{len(n)}/{n}"
    if len(n) == 3:
        return f"3/{n[0]}/{n}"
    return f"{n[:2]}/{n[2:4]}/{n}"


def registry_state(name, version):
    """`(name_exists, version_exists)` from the sparse index, trying a failed request again."""
    url = f"{INDEX}/{index_path(name)}"
    for attempt in range(6):
        try:
            status, body = http_get(url)
        except (urllib.error.URLError, OSError):
            status, body = 0, ""
        if status == 404:
            return False, False
        if status == 200:
            versions = {json.loads(l)["vers"] for l in body.splitlines() if l.strip()}
            return True, version in versions
        sleep(5 * (attempt + 1))
    raise Stop(f"crates.io's index did not answer for {name} after 6 tries")


def vcs_sha(name, version):
    """The commit the published `.crate`'s `.cargo_vcs_info.json` names, or None when it cannot be read."""
    req = urllib.request.Request(f"{DOWNLOAD}/{name}/{name}-{version}.crate", headers={"User-Agent": UA})
    try:
        with urllib.request.urlopen(req, timeout=60) as r:
            data = r.read()
        with tarfile.open(fileobj=io.BytesIO(data), mode="r:gz") as t:
            f = t.extractfile(f"{name}-{version}/.cargo_vcs_info.json")
            return json.load(f)["git"]["sha1"] if f else None
    except (urllib.error.URLError, OSError, tarfile.TarError, KeyError, ValueError):
        return None


def workspace(root):
    """`(packages, deps)`: name -> version, and name -> publishable workspace crates it depends on."""
    out = run(["cargo", "metadata", "--no-deps", "--format-version", "1", "--locked"], root)
    if out.returncode:
        raise Stop("cargo metadata failed:\n" + out.stderr)
    meta = json.loads(out.stdout)
    members = set(meta["workspace_members"])
    pk = {
        p["name"]: p
        for p in meta["packages"]
        if p["id"] in members and (p.get("publish") is None or "crates-io" in p["publish"])
    }
    versions = {n: p["version"] for n, p in pk.items()}
    deps = {
        n: {d["name"] for d in p["dependencies"]
            if d["name"] in pk and (d.get("kind") != "dev" or d.get("req", "*") != "*")}
        for n, p in pk.items()
    }
    return versions, deps


def next_ready(versions, deps, published, existing):
    ready = sorted(n for n in versions if n not in published and deps[n] <= published)
    ready.sort(key=lambda n: (n not in existing, n))
    return ready[0] if ready else None


def order(versions, deps, published, existing):
    """The order `next_ready` yields while every crate in turn becomes published."""
    seq, sim = [], set(published)
    while len(sim) < len(versions):
        n = next_ready(versions, deps, sim, existing)
        if n is None:
            raise Stop("a dependency cycle, or a dependency that is not publishable")
        seq.append(n)
        sim.add(n)
    return seq


def wall_minutes(new, existing):
    """Minutes the two limits impose on this many uploads of each kind, beyond the bursts."""
    return (max(0, new - NEW_BURST) * NEW_MINUTES
            + max(0, existing - EXISTING_BURST) * EXISTING_MINUTES)


def survey(versions):
    published, existing = set(), set()
    for n in sorted(versions):
        name_known, version_known = registry_state(n, versions[n])
        if version_known:
            published.add(n)
        elif name_known:
            existing.add(n)
    return published, existing


def core_group(root):
    path = pathlib.Path(root) / release_check.CONFIG
    names = release_check.read_group(path.read_text(encoding="utf-8"))
    if not names:
        raise Stop(f"{release_check.CONFIG} has no crate in group `{release_check.GROUP}`")
    return set(names)


def core_tag(versions):
    return f"v{versions[release_check.ANCHOR]}"


def tag_commit(root, tag):
    out = run(["git", "rev-parse", "-q", "--verify", f"refs/tags/{tag}^{{commit}}"], root)
    return out.stdout.strip() if out.returncode == 0 else None


def head(root):
    return run(["git", "rev-parse", "HEAD"], root).stdout.strip()


def check_tree(root, versions, need_tag):
    status = run(["git", "status", "--porcelain"], root)
    if status.returncode:
        raise Stop(f"git status failed in {root}:\n{status.stderr}")
    if status.stdout.strip():
        raise Stop("the working tree is not clean, untracked files included, so the commit would not be "
                   "what was checked: publish from a clean checkout of the release commit (RELEASING.md)")
    if not need_tag:
        return
    tag = core_tag(versions)
    at = tag_commit(root, tag)
    if at is None:
        raise Stop(f"the core tag {tag} does not exist: tag the release commit first (RELEASING.md)")
    if at != head(root):
        raise Stop(f"HEAD is {head(root)}, but {tag} names {at}: not publishing")


def ensure_tag(root, name):
    at = tag_commit(root, name)
    if at == head(root):
        return
    if at is not None:
        raise Stop(f"tag {name} exists at {at}, not HEAD")
    out = run(["git", "tag", "-a", name, "-m", name, "HEAD"], root)
    if out.returncode:
        raise Stop(f"git tag {name} failed:\n{out.stdout}{out.stderr}")
    print(f"   tagged {name} (local; not pushed)", flush=True)


def tag_leaf(root, name, version, uploaded):
    """Tags a leaf at HEAD; one this run did not upload only when its published `.crate` names HEAD."""
    tag = f"{name}-v{version}"
    if not uploaded:
        at = vcs_sha(name, version)
        if at != head(root):
            print(f"   {name} {version} on crates.io was packaged from {at or 'a commit its .crate does not name'}, "
                  f"not HEAD: {tag} not created", flush=True)
            return
    ensure_tag(root, tag)


def publish_one(root, name):
    """Uploads one crate, sleeping out a 429 and retrying it. Returns whether this run uploaded it."""
    while True:
        print(f"== publishing {name}", flush=True)
        out = run(["cargo", "publish", "-p", name, "--locked"], root)
        text = out.stdout + out.stderr
        if out.returncode == 0:
            print(f"   published {name}", flush=True)
            return True
        if ALREADY.search(text):
            print(f"   {name} is already on crates.io", flush=True)
            return False
        if is_429(text):
            when = retry_after(text)
            if when is None:
                raise Stop(f"429 for {name} without a retry time:\n{text}")
            wait = wait_seconds(when, now())
            print(f"   429: try again after {when:%H:%M:%S} GMT; sleeping {int(wait)}s", flush=True)
            sleep(wait)
            continue
        raise Stop(f"publishing {name} failed, stopping:\n{text}")


def cmd_plan(root):
    versions, deps = workspace(root)
    published, existing = survey(versions)
    todo = order(versions, deps, published, existing)
    new = [n for n in todo if n not in existing]
    old = [n for n in todo if n in existing]
    print(f"{len(versions)} publishable crates: {len(published)} already published at their version, "
          f"{len(todo)} to go ({len(old)} existing names, {len(new)} new names)")
    print("order (nothing is published by `plan`):")
    for i, n in enumerate(todo, 1):
        print(f"  {i:2}. {n} {versions[n]}   ({'new name' if n in new else 'existing name'})")
    mins = wall_minutes(len(new), len(old))
    print(f"expected wall time from the limits alone: {mins // 60} h {mins % 60} min "
          f"(new names: burst {NEW_BURST}, then 1 per {NEW_MINUTES} min; "
          f"existing: burst {EXISTING_BURST}, then 1 per {EXISTING_MINUTES} min), plus each crate's own build")
    core = core_group(root)
    tags = [core_tag(versions)] + [f"{n}-v{versions[n]}" for n in sorted(versions) if n not in core]
    at = tag_commit(root, tags[0])
    where = "at HEAD" if at == head(root) else (f"at {at}, not HEAD" if at else "absent")
    print(f"local tags `publish` expects or creates: {len(tags)}; the core tag {tags[0]}, created by hand, is {where}")


def cmd_publish(root, dry_run):
    versions, deps = workspace(root)
    check_tree(root, versions, need_tag=not dry_run)
    core = core_group(root)
    published, existing = survey(versions)
    print(f"{len(published)} published, {len(versions) - len(published)} to go", flush=True)
    if dry_run:
        todo = order(versions, deps, published, existing)
        if not todo:
            print("nothing left to check")
            return
        args = ["cargo", "publish", "--workspace", "--dry-run", "--locked"]
        for n in sorted(published):
            args += ["--exclude", n]
        print(f"   packaging and verifying {len(todo)} crates together; each is built", flush=True)
        out = run(args, root)
        if out.returncode:
            raise Stop(f"dry run failed:\n{out.stdout}{out.stderr}")
        print(f"   dry run of {len(todo)} crates passed", flush=True)
        return
    for n in sorted(published):
        if n not in core:
            tag_leaf(root, n, versions[n], uploaded=False)
    while True:
        n = next_ready(versions, deps, published, existing)
        if n is None:
            break
        uploaded = publish_one(root, n)
        published.add(n)
        if n not in core:
            tag_leaf(root, n, versions[n], uploaded)
    print(f"all {len(versions)} crates are on crates.io")


class Fake:
    """Stands in for cargo, git, the sparse index, the clock and sleep. A publish reaches it only
    through `run`, so nothing here can upload a crate."""

    def __init__(self):
        self.head = "c0ffee"
        self.dirty = False
        self.tags = {"v1.0.0": "c0ffee"}
        self.registry = {"ulo-macros": {"0.9.0"}, "leaf-z": {"0.9.0"}}
        self.outcomes = {}
        self.calls = []
        self.sleeps = []
        self.clock = datetime.datetime(2026, 1, 1, 12, 0, 0, tzinfo=datetime.timezone.utc)
        self.fail_http = 0
        self.packaged_from = {}

    def metadata(self):
        def pkg(name, deps=(), dev=(), vdev=(), publish=None):
            return {
                "id": name, "name": name, "version": "1.0.0", "publish": publish,
                "dependencies": [{"name": d, "kind": None, "req": "=1.0.0"} for d in deps]
                + [{"name": d, "kind": "dev", "req": "*"} for d in dev]
                + [{"name": d, "kind": "dev", "req": "=1.0.0"} for d in vdev],
            }
        pk = [
            pkg("ulo", ["ulo-macros", "serde"]),
            pkg("ulo-macros", dev=["leaf-y"]),
            pkg("leaf-x", ["ulo"]),
            pkg("leaf-y", ["leaf-x"]),
            pkg("leaf-z"),
            pkg("leaf-a"),
            pkg("leaf-b", vdev=["leaf-y"]),
            pkg("ex-private", ["ulo"], publish=[]),
        ]
        return json.dumps({"workspace_members": [p["id"] for p in pk], "packages": pk})

    def run(self, args, cwd):
        self.calls.append(list(args))
        ok = subprocess.CompletedProcess(args, 0, "", "")
        if args[:2] == ["cargo", "metadata"]:
            return subprocess.CompletedProcess(args, 0, self.metadata(), "")
        if args[:2] == ["cargo", "publish"]:
            if "--dry-run" in args:
                return ok
            name = args[args.index("-p") + 1]
            queue = self.outcomes.get(name, [])
            res = queue.pop(0) if queue else ok
            if res.returncode == 0:
                self.registry.setdefault(name, set()).add("1.0.0")
            return res
        if args[:3] == ["git", "status", "--porcelain"]:
            return subprocess.CompletedProcess(args, 0, " M x\n" if self.dirty else "", "")
        if args[:2] == ["git", "rev-parse"] and args[-1] == "HEAD":
            return subprocess.CompletedProcess(args, 0, self.head + "\n", "")
        if args[:2] == ["git", "rev-parse"]:
            tag = args[-1][len("refs/tags/"):-len("^{commit}")]
            if tag in self.tags:
                return subprocess.CompletedProcess(args, 0, self.tags[tag] + "\n", "")
            return subprocess.CompletedProcess(args, 1, "", "")
        if args[:3] == ["git", "tag", "-a"]:
            self.tags[args[3]] = self.head
            return ok
        raise AssertionError(f"unexpected command {args}")

    def http_get(self, url):
        if self.fail_http:
            self.fail_http -= 1
            return 503, ""
        name = url.rsplit("/", 1)[1]
        if name not in self.registry:
            return 404, ""
        return 200, "\n".join(json.dumps({"name": name, "vers": v}) for v in sorted(self.registry[name]))

    def vcs_sha(self, name, version):
        return self.packaged_from.get(name, self.head)

    def uploads(self):
        return [c[c.index("-p") + 1] for c in self.calls
                if c[:2] == ["cargo", "publish"] and "--dry-run" not in c]


def failed(returncode, text):
    return subprocess.CompletedProcess([], returncode, "", text)


def stops(fn):
    try:
        fn()
    except Stop as e:
        return str(e)
    return None


def main_quiet(argv):
    import contextlib
    import io
    with contextlib.redirect_stderr(io.StringIO()):
        return main(argv)


def self_test():
    """Runs the checks with every route to a new process replaced by one that raises, so a check that
    reaches past the fakes fails instead of running cargo or git."""
    def forbidden(*a, **k):
        raise AssertionError("the self-test started a real process")
    saved = {}
    for mod, names in ((subprocess, ("Popen", "run", "call", "check_call", "check_output")),
                       (os, ("system", "popen", "fork", "posix_spawn", "posix_spawnp",
                             "execv", "execve", "execvp", "execvpe", "spawnv", "spawnve", "spawnvp", "spawnvpe"))):
        for name in names:
            if hasattr(mod, name):
                saved[(mod, name)] = getattr(mod, name)
                setattr(mod, name, forbidden)
    try:
        return checks()
    finally:
        for (mod, name), v in saved.items():
            setattr(mod, name, v)


def checks():
    import contextlib
    import io
    import tempfile

    this = sys.modules[__name__]
    failures = []
    expected_order = ["leaf-z", "ulo-macros", "leaf-a", "ulo", "leaf-x", "leaf-y", "leaf-b"]

    def check(label, cond, detail=""):
        if not cond:
            failures.append(f"{label} {detail}".strip())

    def scenario(body, setup=None):
        fake = Fake()
        if setup:
            setup(fake)
        saved = {k: getattr(this, k) for k in ("run", "http_get", "sleep", "now", "vcs_sha")}
        this.run, this.http_get, this.vcs_sha = fake.run, fake.http_get, fake.vcs_sha
        this.sleep = lambda s: (fake.sleeps.append(s), setattr(fake, "clock", fake.clock + datetime.timedelta(seconds=s)))
        this.now = lambda: fake.clock
        out = io.StringIO()
        try:
            with tempfile.TemporaryDirectory() as d, contextlib.redirect_stdout(out):
                (pathlib.Path(d) / release_check.CONFIG).write_text(
                    '[[package]]\nname = "ulo"\nversion_group = "core"\n'
                    '[[package]]\nname = "ulo-macros"\nversion_group = "core"\n',
                    encoding="utf-8")
                result = body(fake, pathlib.Path(d))
        finally:
            for k, v in saved.items():
                setattr(this, k, v)
        return fake, out.getvalue(), result

    # Cargo's wrapping of crates.io's answer, whose date is written `%a, %d %b %Y %T GMT`.
    sample = ("the remote server responded with an error (status 429 Too Many Requests): You have published "
              "too many new crates in a short period of time. Please try again after Tue, 03 Nov 2026 08:47:52 GMT "
              "and see https://crates.io/docs/rate-limits for more details.")
    when = retry_after(sample)
    check("429 parse", when is not None and when.isoformat() == "2026-11-03T08:47:52+00:00", when)
    check("non-429 text has no retry time", retry_after("error: something else") is None)
    check("429 detection", is_429(sample) and not is_429("error: other"))
    check("sleep until the time plus margin",
          wait_seconds(when, when - datetime.timedelta(seconds=100)) == 130.0)
    check("a past time sleeps the margin only",
          wait_seconds(when, when + datetime.timedelta(seconds=100)) == float(MARGIN))
    check("index paths", [index_path(n) for n in ("a", "ab", "abc", "ulo", "ulo-http")]
          == ["1/a", "2/ab", "3/a/abc", "3/u/ulo","ul/o-/ulo-http"])
    check("already-published answers recognised",
          ALREADY.search("error: crate ulo@1.0.0 already exists on crates.io index") is not None
          and ALREADY.search("the remote server responded with an error: crate version `1.0.0` is already uploaded")
          is not None)
    check("other failures naming something that already exists are not",
          ALREADY.search("error: failed to create directory: File already exists (os error 17)") is None)
    check("wall time", wall_minutes(5, 30) == 0 and wall_minutes(8, 32) == 32 and wall_minutes(30, 2) == 250)

    # Ordering, and what `cargo metadata` contributes.
    def body_order(fake, root):
        versions, deps = workspace(root)
        published, existing = survey(versions)
        return versions, deps, published, existing, order(versions, deps, published, existing)
    fake, _, (versions, deps, published, existing, seq) = scenario(body_order)
    check("unpublishable crate left out", "ex-private" not in versions, sorted(versions))
    check("dev dependency imposes no order", "leaf-y" not in deps["ulo-macros"], deps["ulo-macros"])
    check("external dependency imposes no order", deps["ulo"] == {"ulo-macros"}, deps["ulo"])
    check("dev dependency naming a version imposes order", deps["leaf-b"] == {"leaf-y"}, deps["leaf-b"])
    check("existing before new, dependencies first", seq == expected_order, seq)
    check("existing names found", existing == {"ulo-macros", "leaf-z"}, existing)
    cyc = {"a": {"b"}, "b": {"a"}}
    check("a cycle is refused", stops(lambda: order({"a": "1", "b": "1"}, cyc, set(), set())) is not None)
    check("a crate waits for its dependency even when existing",
          next_ready({"a": 1, "b": 1}, {"a": set(), "b": {"a"}}, set(), {"b"}) == "a")

    # Index outage: retried, then refused.
    _, _, res = scenario(lambda f, r: registry_state("leaf-z", "0.9.0"), lambda f: setattr(f, "fail_http", 2))
    check("index retried after a 503", res == (True, True), res)
    _, _, res = scenario(lambda f, r: stops(lambda: registry_state("leaf-z", "0.9.0")),
                         lambda f: setattr(f, "fail_http", 99))
    check("index outage refused", res is not None)

    # Plan: reads, publishes nothing.
    fake, out, _ = scenario(lambda f, r: cmd_plan(r))
    check("plan lists the order", all(f"{i}. {n} 1.0.0" in out for i, n in enumerate(expected_order, 1)), out)
    check("plan names the split", "5 new names" in out and "2 existing names" in out, out)
    check("plan prints the wall time", "expected wall time" in out and "0 h 0 min" in out, out)
    check("plan publishes nothing", fake.uploads() == [] and not any("--dry-run" in c for c in fake.calls), fake.calls)
    check("plan tags nothing", set(fake.tags) == {"v1.0.0"}, fake.tags)

    # Publish.
    fake, out, _ = scenario(lambda f, r: cmd_publish(r, False))
    check("publish order", fake.uploads() == expected_order, fake.uploads())
    check("every cargo call is --locked", all("--locked" in c for c in fake.calls if c[0] == "cargo"), fake.calls)
    check("leaf tags created at HEAD", {t for t in fake.tags if t != "v1.0.0"}
          == {"leaf-a-v1.0.0", "leaf-b-v1.0.0", "leaf-x-v1.0.0", "leaf-y-v1.0.0", "leaf-z-v1.0.0"}, fake.tags)
    check("core crates carry no tag of their own", "ulo-v1.0.0" not in fake.tags and "ulo-macros-v1.0.0" not in fake.tags)
    check("no push", not any(c[:2] == ["git", "push"] for c in fake.calls))

    # Already published: skipped, still tagged.
    fake, _, _ = scenario(lambda f, r: cmd_publish(r, False), lambda f: f.registry.update({"leaf-z": {"1.0.0"}}))
    check("published version skipped", fake.uploads() == expected_order[1:], fake.uploads())
    check("skipped leaf still tagged", "leaf-z-v1.0.0" in fake.tags)
    fake, out, _ = scenario(lambda f, r: cmd_publish(r, False),
                            lambda f: (f.registry.update({"leaf-z": {"1.0.0"}}), f.packaged_from.update({"leaf-z": "old"})))
    check("a leaf packaged from another commit is not tagged",
          "leaf-z-v1.0.0" not in fake.tags and "packaged from old" in out and fake.uploads() == expected_order[1:], out)

    # Cargo's own already-uploaded answer, as a rerun meets it when the index lags.
    fake, _, _ = scenario(lambda f, r: cmd_publish(r, False),
                          lambda f: f.outcomes.update({"ulo": [failed(101, "error: crate ulo@1.0.0 already exists on crates.io index")]}))
    check("already-exists answer continues", fake.uploads() == expected_order, fake.uploads())

    # 429: sleep until the named time plus the margin, retry the same crate.
    def limit(f):
        text = "status 429 Too Many Requests ... try again after " + email.utils.format_datetime(
            f.clock + datetime.timedelta(seconds=100), usegmt=True)
        f.outcomes["leaf-x"] = [failed(101, text)]
    fake, out, _ = scenario(lambda f, r: cmd_publish(r, False), limit)
    check("429 slept to the named time plus margin", fake.sleeps == [130.0], fake.sleeps)
    check("429 retried the same crate",
          fake.uploads() == expected_order[:4] + ["leaf-x", "leaf-x", "leaf-y", "leaf-b"], fake.uploads())
    fake, _, res = scenario(lambda f, r: stops(lambda: cmd_publish(r, False)),
                            lambda f: f.outcomes.update({"leaf-x": [failed(101, "429 Too Many Requests")]}))
    check("429 without a time stops", res is not None and fake.sleeps == [], res)

    # Another failure stops the run and leaves the rest alone; a rerun resumes.
    def interrupted(fake, root):
        fake.outcomes["leaf-x"] = [failed(101, "error: network unreachable")]
        first = stops(lambda: cmd_publish(root, False))
        first_uploads = fake.uploads()
        fake.calls.clear()
        cmd_publish(root, False)
        return first, first_uploads, fake.uploads()
    _, _, (first, first_uploads, second_uploads) = scenario(interrupted)
    check("failure stops the run", first is not None and first_uploads == expected_order[:4] + ["leaf-x"], first_uploads)
    check("rerun resumes", second_uploads == ["leaf-x", "leaf-y", "leaf-b"], second_uploads)

    def rerun_done(fake, root):
        cmd_publish(root, False)
        fake.calls.clear()
        cmd_publish(root, False)
    fake, _, _ = scenario(rerun_done)
    check("rerun of a finished release uploads nothing", fake.uploads() == [], fake.uploads())

    # The checks before anything is uploaded.
    def refused(setup, label):
        fake, _, res = scenario(lambda f, r: stops(lambda: cmd_publish(r, False)), setup)
        check(label, res is not None and fake.uploads() == [] and not any(c[:2] == ["git", "tag"] for c in fake.calls), res)
    refused(lambda f: setattr(f, "dirty", True), "dirty tree refused")
    refused(lambda f: setattr(f, "head", "bad"), "HEAD that is not the tagged commit refused")
    refused(lambda f: f.tags.clear(), "missing core tag refused")

    # A leaf tag at another commit stops the run before it goes on.
    fake, _, res = scenario(lambda f, r: stops(lambda: cmd_publish(r, False)),
                            lambda f: f.tags.update({"leaf-x-v1.0.0": "other"}))
    check("a leaf tag elsewhere stops", res is not None and "leaf-y" not in fake.uploads(), res)

    # Dry run: one workspace `--dry-run`, no tags, no core tag needed, a dirty tree still refused.
    fake, out, _ = scenario(lambda f, r: cmd_publish(r, True), lambda f: f.tags.clear())
    dry = [c for c in fake.calls if c[:2] == ["cargo", "publish"]]
    check("dry run only dry-runs, the whole workspace at once",
          fake.uploads() == [] and dry == [["cargo", "publish", "--workspace", "--dry-run", "--locked"]], dry)
    check("every cargo call of a dry run is --locked", all("--locked" in c for c in fake.calls if c[0] == "cargo"))
    check("dry run tags nothing", fake.tags == {}, fake.tags)
    fake, _, _ = scenario(lambda f, r: cmd_publish(r, True), lambda f: f.registry.update({"leaf-z": {"1.0.0"}}))
    dry = [c for c in fake.calls if c[:2] == ["cargo", "publish"]]
    check("dry run excludes what is published",
          dry == [["cargo", "publish", "--workspace", "--dry-run", "--locked", "--exclude", "leaf-z"]], dry)
    fake, _, res = scenario(lambda f, r: stops(lambda: cmd_publish(r, True)), lambda f: setattr(f, "dirty", True))
    check("dry run refuses a dirty tree", res is not None and not any(c[:2] == ["cargo", "publish"] for c in fake.calls))

    # The command line, through `main`, with the fakes in place.
    def via_main(argv):
        return lambda f, r: main_quiet([a.replace("ROOT", str(r)) for a in argv])
    fake, _, rc = scenario(via_main(["--root", "ROOT", "publish", "--dry-run"]))
    check("`publish --dry-run` uploads nothing",
          rc == 0 and fake.uploads() == [] and any("--dry-run" in c for c in fake.calls), (rc, fake.calls))
    fake, _, rc = scenario(via_main(["--root", "ROOT", "plan"]))
    check("`plan` runs no cargo publish", rc == 0 and not any(c[:2] == ["cargo", "publish"] for c in fake.calls))
    fake, _, rc = scenario(via_main(["--root", "ROOT", "publish"]))
    check("`publish` uploads", rc == 0 and fake.uploads() == expected_order, (rc, fake.uploads()))
    for argv in ([], ["publish", "--yes"], ["publish", "--dry-run", "x"], ["plan", "--dry-run"], ["Publish"],
                 ["publish", "--dryrun"], ["--dry-run", "publish"], ["publish", "--root"],
                 ["publish", "--root", "--dry-run"]):
        fake, _, rc = scenario(via_main(argv))
        check(f"usage error {argv}", rc == 2 and fake.calls == [], (rc, fake.calls))

    for f in failures:
        print(f"self-test: {f}", file=sys.stderr)
    return not failures


def main(argv):
    if argv == ["--self-test"]:
        if not self_test():
            return 1
        print("self-test passed")
        return 0
    root, rest = pathlib.Path.cwd(), list(argv)
    if "--root" in rest:
        i = rest.index("--root")
        if i + 1 >= len(rest) or rest[i + 1].startswith("-"):
            print(__doc__, file=sys.stderr)
            return 2
        root = pathlib.Path(rest[i + 1]).resolve()
        del rest[i:i + 2]
    try:
        if rest == ["plan"]:
            cmd_plan(root)
            return 0
        if rest and rest[0] == "publish" and set(rest[1:]) <= {"--dry-run"}:
            cmd_publish(root, "--dry-run" in rest)
            return 0
    except (Stop, release_check.UsageError) as e:
        print(e, file=sys.stderr)
        return 1
    print(__doc__, file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.stdout.reconfigure(line_buffering=True)
    sys.exit(main(sys.argv[1:]))
