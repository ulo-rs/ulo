# ulo-grpc-interop

The gRPC project's interop `TestService` served by `ulo-grpc`, and the runner that calls it with the
standard interop client. The crate is unpublished; CI's `grpc interop` job runs it on every pull
request.

The client is grpc-go's, `google.golang.org/grpc/interop/client`. The gRPC project publishes no
prebuilt binary of it for a runner, so it is built from a tag. Locally, with Go 1.25 or later:

```
cargo build -p ulo-grpc-interop
GOBIN=/tmp/interop-client go install google.golang.org/grpc/interop/client@v1.84.0
python3 crates/ulo-grpc-interop/interop/run.py \
  --server target/debug/ulo-grpc-interop \
  --client "/tmp/interop-client/client --server_host={host} --server_port={port} --test_case={case}" \
  --expect 16 --tls
```

The runner starts the server on a free port, runs each case in `interop/cases.txt` as its own client
process and prints one line per case. With `--tls` it runs the list a second time over TLS, against
a CA and a server certificate the binary generates for the run (`ulo-grpc-interop certs DIR`), the
client trusting the CA through `--use_test_ca --ca_file` and refusing a server that negotiates no
ALPN protocol (`GRPC_ENFORCE_ALPN_ENABLED=true`). A case passes when the client exits 0 and logs its
`<name> done` line. The run fails when a case fails without an `allow` entry in `cases.txt`, when
an `allow`ed case passes or fails without the client line its entry quotes, when `--expect` differs
from the length of the list or the number of cases a pass runs, when a pass asked for does not run,
when the client exits 0 for a case name it does not know, when the server exits during the run or
prints no listening line within a minute, and when it announces `http://` on the TLS pass or
`https://` on the plaintext one. `python3 interop/run.py --self-test` checks those verdicts against
planted results.

`allow` entries name a case the framework does not pass, with an `F` number, the client's failure
line and the reason. The client's other cases, and why each is not in the list, are named at the
top of `cases.txt`.

## The server

`proto/grpc/testing/` holds the messages and the two services the standard cases use, with the
field numbers of the gRPC project's own `.proto` files. `src/lib.rs` implements `TestService` after
the project's interop test descriptions: payloads of the requested size, `response_status` failing
a call with the code and message it names, and, on the unary and the full-duplex call,
`x-grpc-test-echo-initial` echoed on the reply's headers and `x-grpc-test-echo-trailing-bin` on
its trailers. `CacheableUnaryCall`, `HalfDuplexCall` and `UnimplementedCall` have no handler, and
`UnimplementedService` is not registered, so a call to any of them answers `UNIMPLEMENTED`.

The binary listens on `HOST` and `PORT` (default `127.0.0.1:10000`) and prints
`listening on http://ADDR`. With `TLS_CERT` and `TLS_KEY` naming a PEM certificate chain and its
key it serves TLS, ALPN `h2`, and prints `listening on https://ADDR`.
