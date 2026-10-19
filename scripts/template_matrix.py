#!/usr/bin/env python3
"""Every project `ulo new` can write, built and tested as a user would.

    python3 scripts/template_matrix.py --list                 # the matrix, one name per line
    python3 scripts/template_matrix.py http grpc rpc-tcp      # the named projects
    python3 scripts/template_matrix.py all                    # every project in the matrix
    python3 scripts/template_matrix.py generate               # `ulo generate` on a fresh http project
    python3 scripts/template_matrix.py --self-test            # the checks, run against planted output

The matrix is every transport with no database (`http`, `ws`, `grpc`, `rpc-tcp`, `rpc-udp`,
`rpc-nats`, `rpc-redis`, `rpc-rabbitmq`, `rpc-mqtt`, `rpc-kafka`) and every database on `http`
(`http+sqlx-postgres` and the ten after it), plus `generate`. The database layer touches `app.rs`,
`database.rs`, the manifest and `config/app.toml` only, so the full cross of 120 adds nothing the
eleven do not. The names are the `ValueEnum`s of `crates/ulo-cli/src/commands/new.rs`, and `--list`
is checked against the CLI's own `--help` before anything runs.

For each project, in a fresh temporary directory:

 1. `ulo new tm-<name> --transport <t> [--db <d>]`, with the binary built from this workspace.
 2. A `[patch.crates-io]` section appended to the generated `Cargo.toml`, pointing every `ulo-*`
    workspace crate at its directory. The generated manifest pins each one exactly (`=<version>`)
    and none is on crates.io, so the patch is what lets the exact pins resolve to this tree.
 3. `cargo generate-lockfile`. The generated project ships no lockfile, so `--locked` has nothing to
    hold the next two commands to until one exists; generating it once makes `cargo test` and
    `cargo clippy` resolve identically, and the third-party versions are the newest the manifest's
    requirements admit, which is what a user's first build gets.
 4. `cargo test --locked`. The project fails when the output has no `test <name> ... ok` line,
    whatever the exit status: `running 0 tests` exits 0. "Ran a test" is `ran_a_test.py`'s
    definition, the one `unsafe_allowed.py miri` counts with. No `--quiet`: it prints dots in place
    of the lines being counted.
 5. `cargo clippy --all-targets --locked -- -D warnings`.

`generate` writes an http project, runs each of the five kinds of `ulo generate` (controller on both
transports), declares each file with the `mod ..;` line the CLI prints in `src/main.rs`, registers
it with the lines the CLI prints at the end of `AppModule::register`, adds the `Cargo.toml` lines
the CLI says the file needs, then runs steps 3 to 5. The `resolver` kind registers nothing (it is
merged into a query root the project does not have), so its file is dead code and that project's
clippy runs `-D warnings -A dead_code`.

All projects share one target directory (`--target-dir`, `target/template-matrix` by default), so
the framework compiles once per run. Standard library only, and Python 3.9.
"""

import argparse
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from ran_a_test import count_ran  # noqa: E402  the definition `unsafe_allowed.py` shares

ROOT = Path(__file__).resolve().parent.parent

TRANSPORTS = [
    "http", "ws", "grpc", "rpc-tcp", "rpc-udp", "rpc-nats", "rpc-redis", "rpc-rabbitmq", "rpc-mqtt", "rpc-kafka",
]
DATABASES = [
    "sqlx-postgres", "sqlx-mysql", "sqlx-sqlite", "seaorm-postgres", "seaorm-mysql", "seaorm-sqlite",
    "diesel-postgres", "diesel-mysql", "diesel-sqlite", "mongodb", "redis",
]
GENERATE = "generate"

# What `ulo generate` is run for, as `(kind, name, extra arguments)`; `controller` is run on both
# of its transports.
GENERATED = [
    ("module", "billing", []),
    ("provider", "mailer", []),
    ("controller", "orders", ["--transport", "http"]),
    ("controller", "jobs", ["--transport", "rpc"]),
    ("gateway", "chat", []),
    ("resolver", "catalog", []),
]

COMMAND_TIMEOUT = 3600


def matrix():
    """Every project name in the matrix."""
    return TRANSPORTS + [f"http+{db}" for db in DATABASES] + [GENERATE]


def arguments(name):
    """The `ulo new` arguments for a matrix name other than `generate`."""
    if "+" in name:
        transport, db = name.split("+", 1)
        return ["--transport", transport, "--db", db]
    return ["--transport", name]


def project_name(name):
    return "tm-" + name.replace("+", "-")


class Failure(Exception):
    """A project that did not hold, with what it was doing."""


def run(command, cwd, env, log):
    """Runs `command`, appending its output to `log`, and returns the output and exit status."""
    log.write(f"$ {' '.join(map(str, command))}\n")
    log.flush()
    try:
        done = subprocess.run(
            command, cwd=cwd, env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
            errors="replace", timeout=COMMAND_TIMEOUT,
        )
    except subprocess.TimeoutExpired as expired:
        out = expired.stdout.decode("utf-8", "replace") if isinstance(expired.stdout, bytes) else (expired.stdout or "")
        log.write(out + f"\ntimed out after {COMMAND_TIMEOUT}s\n")
        raise Failure(f"`{' '.join(map(str, command))}` timed out after {COMMAND_TIMEOUT}s")
    log.write(done.stdout + f"\nexit {done.returncode}\n")
    log.flush()
    return done.stdout, done.returncode


def must(command, cwd, env, log):
    out, code = run(command, cwd, env, log)
    if code != 0:
        raise Failure(f"`{' '.join(map(str, command))}` exited {code}")
    return out


def workspace_crates():
    """The `ulo-*` workspace crates and their directories, read from `cargo metadata`."""
    import json

    out = subprocess.run(
        ["cargo", "metadata", "--no-deps", "--format-version", "1", "--locked"],
        cwd=ROOT, check=True, capture_output=True, text=True,
    ).stdout
    crates = {}
    for package in json.loads(out)["packages"]:
        if package["name"] == "ulo" or package["name"].startswith("ulo-"):
            crates[package["name"]] = Path(package["manifest_path"]).parent
    return crates


def patch_section(crates):
    lines = ["", "[patch.crates-io]"]
    for name in sorted(crates):
        lines.append(f'{name} = {{ path = "{crates[name]}" }}')
    return "\n".join(lines) + "\n"


def build_cli(env, log):
    """The `ulo` binary, built without `ulo dev`'s watchexec tree: scaffolding is all that is run."""
    must(["cargo", "build", "-p", "ulo-cli", "--no-default-features", "--locked"], ROOT, env, log)
    target = Path(env.get("CARGO_TARGET_DIR") or ROOT / "target")
    binary = target / "debug" / "ulo"
    if not binary.is_file():
        raise Failure(f"the CLI was built but {binary} is no file")
    return binary


def check_test_output(out):
    """The finding for a `cargo test` output that ran no test, `None` when one ran."""
    ran = count_ran(out.splitlines())
    if ran == 0:
        return "`cargo test` ran no test: no `test <name> ... ok` line in its output"
    return None


def check_project(directory, env, log, clippy_args):
    """Steps 3 to 5 in `directory`; raises `Failure` on the first that does not hold."""
    must(["cargo", "generate-lockfile"], directory, env, log)
    out = must(["cargo", "test", "--locked"], directory, env, log)
    finding = check_test_output(out)
    if finding:
        raise Failure(finding)
    ran = count_ran(out.splitlines())
    must(["cargo", "clippy", "--all-targets", "--locked", "--", *clippy_args], directory, env, log)
    return ran


def new_project(ulo, name, base, env, log, patch):
    must([str(ulo), "new", project_name(name), *arguments(name)], base, env, log)
    directory = base / project_name(name)
    with open(directory / "Cargo.toml", "a") as manifest:
        manifest.write(patch)
    return directory


# `ulo generate`'s output, in the sections `generate.rs` prints.
DECLARE = "Declare it where the project's modules are"
REGISTER = "Register it in the module that serves it:"
REQUIRES = "It needs these in Cargo.toml:"


def parse_generated(out):
    """`(mod lines, registration lines, Cargo.toml lines)` from what `ulo generate` printed."""
    section = None
    found = {"declare": [], "register": [], "requires": []}
    for raw in out.splitlines():
        if raw.startswith(DECLARE):
            section = "declare"
        elif raw.startswith(REGISTER):
            section = "register"
        elif raw.startswith(REQUIRES):
            section = "requires"
        elif raw.startswith("    ") and section:
            found[section].append(raw.strip())
        elif not raw.strip():
            continue
        elif not raw.startswith("    "):
            section = None
    return found["declare"], found["register"], found["requires"]


def declare_module(main_rs, lines):
    """`main_rs` with each `mod ..;` line added after the last `mod` declaration it holds."""
    existing = main_rs.split("\n")
    at = max((i for i, line in enumerate(existing) if re.match(r"^mod \w+;$", line)), default=None)
    if at is None:
        raise Failure("src/main.rs declares no module to add after")
    return "\n".join(existing[: at + 1] + lines + existing[at + 1 :])


def register_lines(app_rs, lines):
    """`app_rs` with each registration line added before the closing brace of `register`."""
    start = app_rs.find("fn register(")
    if start == -1:
        raise Failure("src/app.rs has no `fn register`")
    # The method is indented one level in an `impl`, so its closing brace is the next `\n    }`.
    end = app_rs.find("\n    }\n", start)
    if end == -1:
        raise Failure("src/app.rs: no closing brace found for `register`")
    body = "".join(f"\n        {line}" for line in lines)
    return app_rs[:end] + body + app_rs[end:]


def add_dependencies(manifest, lines):
    """`manifest` with each line added at the end of its `[dependencies]` table."""
    head, sep, tail = manifest.partition("\n[dependencies]\n")
    if not sep:
        raise Failure("Cargo.toml has no [dependencies] table")
    # The table ends at the next section header, or at the end of the file.
    m = re.search(r"^\[", tail, re.MULTILINE)
    cut = m.start() if m else len(tail)
    body = tail[:cut].rstrip("\n") + "\n" + "\n".join(lines) + "\n"
    rest = tail[cut:]
    return head + sep + body + ("\n" + rest if rest else "")


def run_generate(ulo, base, env, log, patch):
    directory = new_project(ulo, "http", base, env, log, patch)
    dead_code = False
    for kind, name, extra in GENERATED:
        out = must([str(ulo), "generate", kind, name, *extra], directory, env, log)
        declare, register, requires = parse_generated(out)
        if len(declare) != 1 or not declare[0].startswith("mod "):
            raise Failure(f"`ulo generate {kind}` printed no single `mod` line: {declare}")
        main_rs = directory / "src" / "main.rs"
        main_rs.write_text(declare_module(main_rs.read_text(), declare))
        # A line starting `//` is the CLI telling a person what to do, which nothing here can do.
        code = [line for line in register if not line.startswith("//")]
        if len(code) != len(register):
            dead_code = True
        app_rs = directory / "src" / "app.rs"
        app_rs.write_text(register_lines(app_rs.read_text(), code))
        if requires:
            manifest = directory / "Cargo.toml"
            text = manifest.read_text()
            # The patch section was appended last; the dependency lines go above it.
            text = add_dependencies(text, [line for line in requires if line not in text])
            manifest.write_text(text)
    return directory, dead_code


def one(name, ulo, base, env, patch, logdir, verbose):
    """Runs one project; returns `(ok, seconds, detail)`."""
    began = time.monotonic()
    logpath = logdir / f"{project_name(name)}.log"
    with open(logpath, "w") as log:
        try:
            if name == GENERATE:
                directory, dead_code = run_generate(ulo, base, env, log, patch)
                clippy = ["-D", "warnings", "-A", "dead_code"] if dead_code else ["-D", "warnings"]
            else:
                directory = new_project(ulo, name, base, env, log, patch)
                clippy = ["-D", "warnings"]
            ran = check_project(directory, env, log, clippy)
            detail = f"{ran} tests ran"
            ok = True
        except Failure as failure:
            detail, ok = str(failure), False
    seconds = time.monotonic() - began
    if not ok or verbose:
        print(f"::group::{name} log" if os.environ.get("GITHUB_ACTIONS") else f"--- {name}: {logpath}")
        print(logpath.read_text())
        if os.environ.get("GITHUB_ACTIONS"):
            print("::endgroup::")
    return ok, seconds, detail


def cli_names(ulo, env):
    """The values `--transport` and `--db` accept, read from the CLI's `--help`."""
    help_text = subprocess.run([str(ulo), "new", "--help"], capture_output=True, text=True, env=env).stdout
    found = {}
    # One block per option, each starting at its `--flag` line; a block lists its values either as
    # `- name: description` lines or as `[possible values: a, b]`.
    for block in re.split(r"(?m)^      (?=--)", help_text)[1:]:
        flag = re.match(r"--([a-z-]+)", block).group(1)
        listed = re.search(r"\[possible values: ([^\]]*)\]", block)
        found[flag] = (
            [v.strip() for v in listed.group(1).split(",")] if listed else re.findall(r"(?m)^\s*- ([a-z0-9-]+):", block)
        )
    return found


def self_test():
    """The checks run against output known to violate them and output known to satisfy them."""
    failures = []

    def expect(label, got, want):
        if got != want:
            failures.append(f"{label}: got {got!r}, wanted {want!r}")

    valid = "running 2 tests\ntest greeter::tests::greets ... ok\ntest app::tests::wires ... ok\n\ntest result: ok. 2 passed"
    expect("a run with ok lines", check_test_output(valid), None)
    expect("a should_panic test", check_test_output("test a::b - should panic ... ok\n"), None)
    for label, output in [
        ("no tests", "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored"),
        ("only ignored tests", "running 1 test\ntest a::b ... ignored\n\ntest result: ok. 0 passed; 0 failed; 1 ignored"),
        ("quiet dots", "running 2 tests\n..\ntest result: ok. 2 passed"),
        ("empty", ""),
    ]:
        finding = check_test_output(output)
        expect(f"{label} is refused", finding is not None and "ran no test" in finding, True)

    printed = (
        "Created src/jobs_controller.rs\n\n"
        "Declare it where the project's modules are, usually src/main.rs:\n    mod jobs_controller;\n\n"
        "Register it in the module that serves it:\n    m.controller::<crate::jobs_controller::JobsController>();\n\n"
        "It needs these in Cargo.toml:\n    ulo-rpc = \"=0.1.0\"\n"
    )
    expect("parse_generated", parse_generated(printed), (
        ["mod jobs_controller;"],
        ["m.controller::<crate::jobs_controller::JobsController>();"],
        ['ulo-rpc = "=0.1.0"'],
    ))
    expect("declare_module", declare_module("mod app;\nmod greeter;\n\nfn main() {}\n", ["mod x;"]),
           "mod app;\nmod greeter;\nmod x;\n\nfn main() {}\n")
    app = "impl Module for A {\n    fn register(&self, m: &mut ModuleDef<'_>) {\n        m.a();\n    }\n}\n"
    expect("register_lines", register_lines(app, ["m.b();"]),
           "impl Module for A {\n    fn register(&self, m: &mut ModuleDef<'_>) {\n        m.a();\n        m.b();\n    }\n}\n")
    manifest = '[package]\nname = "p"\n\n[dependencies]\nserde = "1"\n\n[build-dependencies]\nb = "1"\n'
    expect("add_dependencies", add_dependencies(manifest, ['ulo-rpc = "=1"']),
           '[package]\nname = "p"\n\n[dependencies]\nserde = "1"\nulo-rpc = "=1"\n\n[build-dependencies]\nb = "1"\n')
    expect("project_name", project_name("http+sqlx-sqlite"), "tm-http-sqlx-sqlite")
    expect("matrix size", len(matrix()), 10 + 11 + 1)

    if failures:
        for failure in failures:
            print("self-test:", failure)
        return 1
    print("self-test: 4 violating outputs refused, 2 valid outputs accepted, the generate edits exact")
    return 0


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("projects", nargs="*", help="matrix names, or `all`")
    parser.add_argument("--list", action="store_true", help="print the matrix and exit")
    parser.add_argument("--self-test", action="store_true", help="run the checks against planted output and exit")
    parser.add_argument("--ulo", help="a built `ulo` binary; built from this workspace when unset")
    parser.add_argument("--target-dir", default=str(ROOT / "target" / "template-matrix"))
    parser.add_argument("--keep", action="store_true", help="keep the temporary directory")
    parser.add_argument("--verbose", action="store_true", help="print every project's log, not only a failure's")
    args = parser.parse_args()

    if args.self_test:
        return self_test()
    if args.list:
        print("\n".join(matrix()))
        return 0
    selected = matrix() if args.projects == ["all"] else args.projects
    unknown = [name for name in selected if name not in matrix()]
    if not selected or unknown:
        print(f"unknown or no project: {unknown or 'none named'}; `--list` shows the matrix", file=sys.stderr)
        return 2

    base = Path(tempfile.mkdtemp(prefix="template-matrix-"))
    logdir = base / "logs"
    logdir.mkdir()
    # NO_COLOR: the CLI colours its output, and the lines parsed here are plain text.
    env = dict(os.environ, CARGO_TARGET_DIR=args.target_dir, NO_COLOR="1", CARGO_TERM_COLOR="never")
    try:
        with open(logdir / "setup.log", "w") as log:
            ulo = Path(args.ulo).resolve() if args.ulo else build_cli(dict(env, CARGO_TARGET_DIR=os.environ.get("CARGO_TARGET_DIR", str(ROOT / "target"))), log)
            patch = patch_section(workspace_crates())
        names = cli_names(ulo, env)
        if names.get("transport") != TRANSPORTS or names.get("db") != DATABASES:
            print(f"the CLI's names differ from the matrix: {names}", file=sys.stderr)
            return 2
        results = []
        for name in selected:
            workdir = base / project_name(name)
            workdir.mkdir()
            ok, seconds, detail = one(name, ulo, workdir, env, patch, logdir, args.verbose)
            results.append((name, ok, seconds, detail))
            print(f"{'ok  ' if ok else 'FAIL'} {name:<28} {seconds:7.1f}s  {detail}", flush=True)
        failed = [name for name, ok, _, _ in results if not ok]
        print(f"{len(results) - len(failed)} of {len(results)} projects held" + (f"; failed: {', '.join(failed)}" if failed else ""))
        return 1 if failed else 0
    finally:
        if args.keep:
            print(f"kept {base}")
        else:
            shutil.rmtree(base, ignore_errors=True)


if __name__ == "__main__":
    sys.exit(main())
