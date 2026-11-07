#!/usr/bin/env python3
"""Runs the gRPC interop client's standard cases against `ulo-grpc-interop` and judges the results.

    python3 run.py --server TARGET/debug/ulo-grpc-interop --client CLIENT --expect 16 [--tls]
    python3 run.py --self-test

`--client` is the client command and its flags, with `{host}`, `{port}` and `{case}` filled in per
case: by default the grpc-go interop client, `client --server_host={host} --server_port={port}
--test_case={case}`, which must be on the command line as `client` or by path. Each pass starts the
server on a free port and closes it at the pass's end. Each case in `cases.txt` runs as its own
client process, so one failure does not hide the next.

The case list runs once over plaintext and, with `--tls`, a second time over TLS. For the TLS pass
the server binary writes a CA and a server certificate for `foo.test.google.fr` into a temporary
directory (`ulo-grpc-interop certs DIR`), the server serves with them, and each client command gains
`--tls-client`, by default grpc-go's `--use_tls=true --use_test_ca=true --ca_file={ca}
--server_host_override=foo.test.google.fr`. That pass runs the client with
`GRPC_ENFORCE_ALPN_ENABLED=true`, under which grpc-go closes a connection whose server selected no
ALPN protocol, so every case passing on it shows the server negotiated `h2`, the one protocol the
client offers.

A case passes when the client exits 0 and logs its `<marker> done` line. The run fails on:

- a case that fails without an `allow` entry in `cases.txt`;
- an `allow`ed case that passes, so an allowance is removed once the framework passes the case;
- an `allow`ed case whose client output lacks the failure line its entry quotes, so the allowance
  covers that one failure and not another one in the same case;
- a case list whose length differs from `--expect`, or a count of cases run in a pass that differs
  from it, or a total of runs other than `--expect` times the passes asked for, so a truncated list,
  an empty one or a skipped pass cannot pass;
- a client that exits 0 for a case name it does not know;
- a server that exits during the run, or that does not print its listening line within a minute;
- a server announcing `http://` on the TLS pass or `https://` on the plaintext one.

Standard library only, and Python 3.9.
"""

import argparse
import collections
import os
import pathlib
import re
import shlex
import signal
import socket
import subprocess
import sys
import tempfile
import threading
import time

HERE = pathlib.Path(__file__).resolve().parent
DEFAULT_CLIENT = "client --server_host={host} --server_port={port} --test_case={case}"
DEFAULT_TLS_CLIENT = "--use_tls=true --use_test_ca=true --ca_file={ca} --server_host_override=foo.test.google.fr"
ALLOW = re.compile(r'^allow\s+(F-NEW-\d+|F\d+)\s+"([^"]+)"\s+(\S.*)$')
BOGUS = "no_such_case_ulo_interop"
LISTEN_WAIT = 60


def parse_cases(text):
    """`[(case, marker, allowance)]`, `allowance` being `(F-number, failure line, reason)` or `None`."""
    cases = []
    for number, raw in enumerate(text.splitlines(), 1):
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        parts = line.split(None, 2)
        if len(parts) < 2:
            raise ValueError(f'cases.txt:{number}: expected `<test_case> <marker> [allow <F-number> "<failure>" <reason>]`')
        allowance = None
        if len(parts) == 3:
            match = ALLOW.match(parts[2])
            if not match:
                raise ValueError(f'cases.txt:{number}: `{parts[2]}` is not `allow <F-number> "<failure>" <reason>`')
            allowance = match.groups()
        cases.append((parts[0], parts[1], allowance))
    names = [case for case, _, _ in cases]
    duplicated = sorted({n for n in names if names.count(n) > 1})
    if duplicated:
        raise ValueError(f"cases.txt lists {duplicated} twice")
    return cases


def passed(code, output, marker):
    """Whether a client run passed its case: exit status 0 and the case's `<marker> done` line."""
    return code == 0 and re.search(rf"\b{re.escape(marker)} done\b", output) is not None


def judge(cases, results, expect, unknown_code, server_exit):
    """Each reason one pass fails. `results` maps a case to `(passed, output)`, `unknown_code` is
    the client's exit status for a case name it does not know, and `server_exit` the server's exit
    status if it exited during the pass."""
    problems = []
    if unknown_code == 0:
        problems.append(f"the client exits 0 for the unknown case `{BOGUS}`, so its exit status says nothing")
    if server_exit is not None:
        problems.append(f"the server exited with status {server_exit} during the run")
    if len(cases) != expect:
        problems.append(f"cases.txt lists {len(cases)} cases, expected {expect}")
    if len(results) != expect:
        problems.append(f"{len(results)} cases ran, expected {expect}")
    for case, _, allowance in cases:
        if case not in results:
            problems.append(f"{case}: did not run")
            continue
        ok, output = results[case]
        if ok and allowance:
            problems.append(f"{case}: passes, but cases.txt allows its failure as {allowance[0]}; remove the allowance")
        elif not ok and not allowance:
            problems.append(f"{case}: failed")
        elif not ok and allowance[1] not in output:
            problems.append(f"{case}: fails without the line `{allowance[1]}` its {allowance[0]} allowance names")
    return problems


def judge_passes(wanted, ran, expect):
    """Each reason the passes together fail: `wanted` names the passes asked for and `ran` maps each
    pass that ran to its results, so a skipped pass, or a total other than `expect` per pass, fails."""
    problems = [f"the {name} pass did not run" for name in wanted if name not in ran]
    total = sum(len(results) for results in ran.values())
    if total != expect * len(wanted):
        problems.append(f"{total} case runs over {len(ran)} passes, expected {expect * len(wanted)}")
    return problems


def judge_scheme(tls, scheme):
    """The reason a pass's server announced the wrong scheme, `None` when it did not."""
    want = "https" if tls else "http"
    return None if scheme == want else f"the server announced `{scheme}://`, expected `{want}://`"


def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def run_case(template, host, port, case, timeout, extra_env):
    """The client's exit status, `None` when it timed out, and its output."""
    command = [part.format(host=host, port=port, case=case) for part in shlex.split(template)]
    env = dict(os.environ, GRPC_GO_LOG_SEVERITY_LEVEL="info", **extra_env)
    try:
        done = subprocess.run(command, capture_output=True, text=True, timeout=timeout, env=env)
    except subprocess.TimeoutExpired:
        return None, f"timed out after {timeout}s"
    return done.returncode, done.stdout + done.stderr


def tail(output, lines=4):
    """The client's FATAL or ERROR lines, else its last few: its INFO log of the channel is noise."""
    all_lines = output.strip().splitlines()
    shown = [l for l in all_lines if re.search(r"\b(FATAL|ERROR)\b|panic", l)] or all_lines[-lines:]
    return "\n".join("      " + l for l in shown)


def start_server(binary, bind, port, extra_env):
    """The server once it prints its listening line, the line's scheme kept as `server.scheme`. Its
    output is read to the end on a thread, so a server that logs never blocks on a full pipe, and
    the last lines are kept for an error."""
    env = dict(os.environ, HOST=bind, PORT=str(port), **extra_env)
    server = subprocess.Popen([binary], stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, env=env)
    listening = threading.Event()
    server.last_lines = collections.deque(maxlen=20)
    server.scheme = None

    def read():
        for line in server.stdout:
            server.last_lines.append(line.rstrip())
            if line.startswith("listening on"):
                if server.scheme is None:
                    server.scheme = line.split()[-1].split("://")[0]
                listening.set()

    threading.Thread(target=read, daemon=True).start()
    deadline = time.monotonic() + LISTEN_WAIT
    while not listening.wait(0.1):
        if server.poll() is not None or time.monotonic() > deadline:
            code = server.poll()
            stop_server(server)
            reason = f"exited with status {code}" if code is not None else f"printed no listening line within {LISTEN_WAIT}s"
            last = "\n".join("      " + l for l in server.last_lines)
            raise RuntimeError(f"the server {reason}" + (f":\n{last}" if last else ""))
    return server


def stop_server(server):
    if server.poll() is not None:
        return
    server.send_signal(signal.SIGTERM)
    try:
        server.wait(timeout=15)
    except subprocess.TimeoutExpired:
        server.kill()
        server.wait()


def run_pass(args, cases, tls, certs):
    """One pass of the case list against a server of its own: `(results, problems)`. Raises
    `RuntimeError` when the server does not start."""
    server_env, client, client_env = {}, args.client, {}
    if tls:
        server_env = {"TLS_CERT": os.path.join(certs, "cert.pem"), "TLS_KEY": os.path.join(certs, "key.pem")}
        client = args.client + " " + args.tls_client.replace("{ca}", os.path.join(certs, "ca.pem"))
        client_env = {"GRPC_ENFORCE_ALPN_ENABLED": "true"}
    port = free_port()
    server = start_server(args.server, args.bind, port, server_env)
    try:
        unknown_code, unknown_output = run_case(client, args.host, port, BOGUS, args.timeout, client_env)
        results = {}
        for case, marker, allowance in cases:
            code, output = run_case(client, args.host, port, case, args.timeout, client_env)
            ok = passed(code, output, marker)
            results[case] = (ok, output)
            status = "pass" if ok else ("FAIL (allowed as %s)" % allowance[0] if allowance else "FAIL")
            print(f"{case:32} {status}")
            if not ok:
                print(tail(output))
        server_exit = server.poll()
    finally:
        stop_server(server)
    problems = judge(cases, results, args.expect, unknown_code, server_exit)
    wrong_scheme = judge_scheme(tls, server.scheme)
    if wrong_scheme:
        problems.append(wrong_scheme)
    if unknown_code == 0:
        print(f"the client's output for `{BOGUS}`:\n{tail(unknown_output)}", file=sys.stderr)
    return results, problems


def write_certs(binary, certs):
    """The TLS pass's CA, certificate and key, written into `certs` by the server binary."""
    made = subprocess.run([binary, "certs", certs], capture_output=True, text=True)
    if made.returncode != 0:
        raise RuntimeError(f"`{binary} certs` exited with status {made.returncode}: {made.stdout}{made.stderr}")


def main(argv):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--server", help="the ulo-grpc-interop binary")
    parser.add_argument("--client", default=DEFAULT_CLIENT, help="the client command, with {host} {port} {case}")
    parser.add_argument("--host", default="127.0.0.1", help="the address the client dials")
    parser.add_argument("--bind", default="127.0.0.1", help="the address the server listens on")
    parser.add_argument("--cases", default=str(HERE / "cases.txt"))
    parser.add_argument("--expect", type=int, help="the number of cases cases.txt lists")
    parser.add_argument("--timeout", type=int, default=60, help="seconds each case may take")
    parser.add_argument("--tls", action="store_true", help="run the case list a second time, over TLS")
    parser.add_argument("--tls-client", default=DEFAULT_TLS_CLIENT, help="the client flags the TLS pass adds, with {ca}")
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args(argv)
    if args.self_test:
        return self_test()
    if not args.server or args.expect is None:
        parser.error("--server and --expect are required")

    cases = parse_cases(pathlib.Path(args.cases).read_text(encoding="utf-8"))
    wanted = ["plaintext"] + (["tls"] if args.tls else [])
    ran, problems = {}, []
    with tempfile.TemporaryDirectory(prefix="ulo-interop-certs-") as certs:
        for name in wanted:
            print(f"== {name}")
            try:
                if name == "tls":
                    write_certs(args.server, certs)
                results, found = run_pass(args, cases, name == "tls", certs)
            except RuntimeError as error:
                print(f"error: {name}: {error}", file=sys.stderr)
                return 1
            ran[name] = results
            problems += [f"{name}: {problem}" for problem in found]
    problems += judge_passes(wanted, ran, args.expect)
    runs = [ok for results in ran.values() for ok, _ in results.values()]
    print(f"\n{len(runs)} case runs over {len(ran)} passes: {sum(runs)} passed, {len(runs) - sum(runs)} failed, "
          f"{sum(1 for _, _, a in cases if a)} allowed per pass")
    for problem in problems:
        print(f"error: {problem}", file=sys.stderr)
    return 1 if problems else 0


def self_test():
    """`parse_cases`, `passed`, `judge`, `judge_passes` and `judge_scheme` against planted input:
    each bad run refused, the good one passed."""
    text = '# c\na A\nb B allow F-NEW-1 "want trailer" not supported\nc C\n'
    cases = parse_cases(text)
    assert [c[0] for c in cases] == ["a", "b", "c"] and cases[1][2] == ("F-NEW-1", "want trailer", "not supported"), cases
    for bad in ("a\n", "a A allow nonsense\n", "a A allow F1 no quoted failure\n", "a A\na A\n"):
        try:
            parse_cases(bad)
        except ValueError:
            continue
        raise AssertionError(f"parse_cases accepted {bad!r}")
    assert passed(0, "x A done\n", "A")
    assert not passed(0, "", "A") and not passed(1, "A done", "A") and not passed(None, "A done", "A")
    assert not passed(0, "BA done", "A")
    good = {"a": (True, ""), "b": (False, "FATAL: want trailer"), "c": (True, "")}
    assert judge(cases, good, 3, 1, None) == []
    refused = [
        (dict(good, c=(False, "")), 3, 1, None, "c: failed"),
        (dict(good, b=(True, "")), 3, 1, None, "b: passes"),
        (dict(good, b=(False, "FATAL: want header")), 3, 1, None, "without the line `want trailer`"),
        ({"a": (True, ""), "b": (False, "want trailer")}, 3, 1, None, "c: did not run"),
        (good, 4, 1, None, "expected 4"),
        ({}, 3, 1, None, "0 cases ran"),
        (good, 3, 0, None, "exits 0 for the unknown case"),
        (good, 3, 1, -9, "server exited with status -9"),
    ]
    for results, expect, unknown_code, server_exit, needle in refused:
        found = judge(cases, results, expect, unknown_code, server_exit)
        assert any(needle in p for p in found), (results, expect, found)
    assert judge_passes(["plaintext", "tls"], {"plaintext": good, "tls": good}, 3) == []
    for wanted, ran, needle in [
        (["plaintext", "tls"], {"plaintext": good}, "the tls pass did not run"),
        (["plaintext", "tls"], {"plaintext": good, "tls": {"a": (True, "")}}, "4 case runs over 2 passes, expected 6"),
        (["plaintext"], {}, "the plaintext pass did not run"),
    ]:
        found = judge_passes(wanted, ran, 3)
        assert any(needle in p for p in found), (wanted, found)
    assert judge_scheme(True, "https") is None and judge_scheme(False, "http") is None
    assert "expected `https://`" in judge_scheme(True, "http") and "expected `http://`" in judge_scheme(False, "https")
    assert "expected `https://`" in judge_scheme(True, None)
    print("self-test ok")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
