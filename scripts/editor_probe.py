#!/usr/bin/env python3
"""Asks a rust-analyzer binary for hover and go-to-definition on the attributes of a Rust file.

usage: editor_probe.py <rust-analyzer> <project root> <file> [--target-dir DIR] [--point NAME:LINE:COL]...

Without --point, every pathed attribute `#[a::b(..)]` in <file> yields two points, one on `a` and
one on `b`, and a bare `#[routes]` or `#[injectable]` one control point. A marker attribute, one
that `#[routes]` or `#[injectable]` reads and removes (`guards`, `interceptors`, `error_handlers`,
`meta`, `construct`, and `injectable(default)` on a field), yields one point on its last segment,
bare, pathed or inside a `#[cfg_attr(..)]`: the editor resolves it through the import the macro
writes for that segment alone. An attribute on a `//` comment line yields none, and neither does
one inside a `macro_rules!` body, which the editor resolves only where the macro is invoked. The server is started over stdio, waited on until it
reports itself quiescent, and each point is asked for `textDocument/hover` and
`textDocument/definition`. A point missing either answer fails the run (exit 1), as does a server
that reports the project failed to load. Lines and columns are 0-based, as LSP counts them.

An outer attribute macro that rebuilds its item through `syn` collapses every delimiter span, and the
editor then answers nothing on the attributes inside it; this is the check that they still answer.

Needs the project to be buildable offline (a language server runs its build scripts and proc macros)
and one rust-analyzer binary: `rustup component add rust-analyzer`, or the one an editor ships.
A --target-dir serves one checkout: cargo names a workspace member's artifacts by its path inside the
workspace and judges them fresh by modification time, so a second checkout of the same workspace
built into the same directory can load the first one's proc macros.
Standard library only, and Python 3.9.
"""

import json
import os
import pathlib
import queue
import re
import signal
import subprocess
import sys
import tempfile
import threading
import time

ATTR = re.compile(r"#\[\s*((?:[A-Za-z_]\w*::)+)([A-Za-z_]\w*)")
BARE = re.compile(r"#\[\s*(routes|injectable)\s*\]")
MARKERS = r"guards|interceptors|error_handlers|meta|construct"
# A marker's last segment: after `#[` or, inside a `cfg_attr`, after a comma, with any path before it.
MARKER = re.compile(r"(?:#\[|,)\s*(?:[A-Za-z_]\w*::)*(?:(" + MARKERS + r")\b(?!\s*::)|(injectable)(?=\s*\(\s*default\s*\)))")


def macro_rules_lines(text):
    """The 0-based lines inside a `macro_rules!` body, braces counted naively."""
    lines = set()
    for m in re.finditer(r"macro_rules!\s*\w+\s*\{", text):
        depth, i = 1, m.end()
        while i < len(text) and depth:
            depth += {"{": 1, "}": -1}.get(text[i], 0)
            i += 1
        first = text.count("\n", 0, m.start())
        lines.update(range(first, first + text.count("\n", m.start(), i) + 1))
    return lines


def points_of(text):
    """(name, line, col) for the first and last segment of every pathed attribute, and the controls."""
    points = []
    skipped = macro_rules_lines(text)
    for line, src in enumerate(text.split("\n")):
        if line in skipped or src.lstrip().startswith("//"):
            continue
        markers = set()
        if "#[" in src:
            for m in MARKER.finditer(src):
                group = 1 if m.group(1) else 2
                points.append((m.group(group), line, m.start(group)))
                markers.add(m.start(group))
        for m in ATTR.finditer(src):
            if m.start(2) in markers:
                continue
            first = m.group(1).split("::")[0]
            points.append((first, line, m.start(1)))
            points.append((m.group(2), line, m.start(2)))
        for m in BARE.finditer(src):
            points.append((m.group(1), line, m.start(1)))
    return sorted(points, key=lambda p: (p[1], p[2]))


class Server:
    def __init__(self, binary, root, target):
        env = {**os.environ, "CARGO_TARGET_DIR": target}
        self.proc = subprocess.Popen(
            [binary], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=open(os.path.join(target, "ra.stderr"), "wb"), env=env
        )
        self.inbox = queue.Queue()
        self.status = {}
        self.replies = {}
        self.next_id = 0
        threading.Thread(target=self._read, daemon=True).start()

    def _read(self):
        out = self.proc.stdout
        while True:
            length = None
            while True:
                line = out.readline()
                if not line:
                    return
                line = line.strip()
                if not line:
                    break
                key, value = line.split(b":", 1)
                if key.lower() == b"content-length":
                    length = int(value)
            self.inbox.put(json.loads(out.read(length)))

    def send(self, message):
        body = json.dumps(message).encode()
        self.proc.stdin.write(b"Content-Length: %d\r\n\r\n" % len(body) + body)
        self.proc.stdin.flush()

    def request(self, method, params):
        self.next_id += 1
        self.send({"jsonrpc": "2.0", "id": self.next_id, "method": method, "params": params})
        return self.next_id

    def pump(self, until, timeout):
        end = time.time() + timeout
        while time.time() < end:
            if until():
                return True
            try:
                m = self.inbox.get(timeout=0.5)
            except queue.Empty:
                continue
            if "method" in m and "id" in m:
                result = [None] * len(m["params"]["items"]) if m["method"] == "workspace/configuration" else None
                self.send({"jsonrpc": "2.0", "id": m["id"], "result": result})
            elif m.get("method") == "experimental/serverStatus":
                self.status.update(m["params"])
            elif "id" in m:
                self.replies[m["id"]] = m
        return until()

    def ask(self, method, params, timeout=120):
        i = self.request(method, params)
        self.pump(lambda: i in self.replies, timeout)
        return self.replies.get(i, {}).get("result")


def main(argv):
    if len(argv) < 4:
        print(__doc__, file=sys.stderr)
        return 2
    binary, root, file = (os.path.abspath(a) for a in argv[1:4])
    target = None
    points = []
    rest = argv[4:]
    while rest:
        flag = rest.pop(0)
        if flag == "--target-dir":
            target = os.path.abspath(rest.pop(0))
        elif flag == "--point":
            name, line, col = rest.pop(0).rsplit(":", 2)
            points.append((name, int(line), int(col)))
        else:
            print("unknown argument", flag, file=sys.stderr)
            return 2
    text = open(file).read()
    points = points or points_of(text)
    if not points:
        print("no points to ask about in", file, file=sys.stderr)
        return 2
    scratch = None
    if target is None:
        scratch = tempfile.TemporaryDirectory(prefix="editor_probe_")
        target = scratch.name
    os.makedirs(target, exist_ok=True)

    server = Server(binary, root, target)
    signal.signal(signal.SIGTERM, lambda *_: sys.exit(143))
    try:
        return probe(server, root, file, text, points, target)
    finally:
        server.proc.kill()
        server.proc.wait()
        if scratch:
            scratch.cleanup()


def probe(server, root, file, text, points, target):
    uri = pathlib.Path(file).as_uri()
    folder = pathlib.Path(root).as_uri()
    init = server.request("initialize", {
        "processId": os.getpid(),
        "rootUri": folder,
        "workspaceFolders": [{"uri": folder, "name": os.path.basename(root)}],
        "capabilities": {
            "experimental": {"serverStatusNotification": True},
            "textDocument": {"hover": {"contentFormat": ["markdown"]}},
            "window": {"workDoneProgress": True},
        },
        "initializationOptions": {"checkOnSave": False, "cargo": {"targetDir": os.path.join(target, "ra")}},
    })
    server.pump(lambda: init in server.replies, 120)
    server.send({"jsonrpc": "2.0", "method": "initialized", "params": {}})
    server.send({"jsonrpc": "2.0", "method": "textDocument/didOpen",
                 "params": {"textDocument": {"uri": uri, "languageId": "rust", "version": 1, "text": text}}})
    started = time.time()
    quiescent = server.pump(lambda: server.status.get("quiescent") and time.time() - started > 10, 1500)
    print("quiescent:", quiescent, "health:", server.status.get("health"), flush=True)
    if server.status.get("health") == "error":
        print("the server failed to load the project:", server.status.get("message"), file=sys.stderr)
        return 1

    def answers():
        rows = []
        for name, line, col in points:
            pos = {"textDocument": {"uri": uri}, "position": {"line": line, "character": col}}
            hover = server.ask("textDocument/hover", pos)
            definition = server.ask("textDocument/definition", pos)
            rows.append((name, line, col, bool(hover), bool(definition)))
        return rows

    rows = answers()
    for _ in range(2):
        if all(r[3] and r[4] for r in rows):
            break
        time.sleep(10)
        rows = answers()
    failed = 0
    print(f"{'point':<14}{'line:col':<10}{'hover':<8}definition")
    for name, line, col, hover, definition in rows:
        print(f"{name:<14}{str(line) + ':' + str(col):<10}{'yes' if hover else 'NO':<8}{'yes' if definition else 'NO'}")
        failed += not (hover and definition)
    print(f"{len(rows) - failed}/{len(rows)} points answered both")
    return 1 if failed or not quiescent else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
