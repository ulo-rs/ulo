# ulo-bench-overhead

Framework overhead: what a `GET /` route costs on ulo's hyper backend beyond the same route on bare
hyper. The crate is unpublished and holds one benchmark.

```
cargo bench -p ulo-bench-overhead
```

Once built, the run takes about a minute and prints each round, then a table of medians:

```
                    req/s       p50 us       p99 us
bare hyper         139822        456.5        569.8
ulo                122858        518.7        900.2
ulo / bare          0.879        1.135        1.580
```

## What is measured

Both servers answer `GET /` with `200`, `content-type: text/plain; charset=utf-8` and the body
`Hello, ulo!`. The harness fetches `/` from each before measuring and fails unless status, headers
(but `date`) and body are equal.

- **Bare hyper:** `hyper::server::conn::http1::Builder` with a tokio timer and the 30-second
  header-read timeout `ulo_http::Server` applies by default, serving each accepted connection with
  `service_fn` and `with_upgrades()`.
- **ulo:** a `#[controller]` with one `#[ulo_http::get("/")]` handler returning `&'static str`,
  bound to `ulo_http_hyper::Server` on `Tokio::current()`. The route declares no guard,
  interceptor or middleware; what the app's service does between hyper and the handler is the
  difference measured. No tracing subscriber is installed, so the span the service opens for each
  call is disabled; an application with a subscriber pays for it.

Each server runs on its own multi-thread tokio runtime with 4 workers. The load loop is one client
runtime with 4 workers and 64 keep-alive HTTP/1.1 connections, each sending the next request when
the previous response has arrived. A round starts a fresh server per target, discards one second
of warm-up, and records every request's latency for five seconds. Rounds alternate which target
goes first. The table reports the median of each column over five rounds, and its `ulo / bare` row
the median of the five rounds' ratios, so a host that slows during the run moves both targets of a
round together.

`ULO_BENCH_WORKERS`, `ULO_BENCH_CONNECTIONS`, `ULO_BENCH_SECONDS` and `ULO_BENCH_ROUNDS` change the
four settings.

The latencies are those of a closed loop at saturation: with 64 requests always in flight, the
mean latency follows from the throughput (64 divided by req/s is about 458 us for bare hyper's
figure), and p50 sits close to it. The p99 is not fixed by the throughput, and its ratio, 1.580,
is larger than the throughput's. It is the tail of queueing at saturation, not the latency of one
request under a fixed load below it.

## Reading the numbers

The figures above are from one machine and one run of the command: an Apple M5 (4 performance
and 6 efficiency cores), 16 GiB, macOS 27.0.1, rustc 1.99.1, the `bench` profile, with the servers
and the client on the same host sharing its cores. They describe that run. On the same host, six
runs put the throughput ratio between 0.82 and 0.88, and one run with other builds running beside it
gave 0.917 with a p99 ratio of 1.123: the figures move with what else the host runs. Other hardware,
core counts, request shapes, payload sizes and HTTP/2 are not measured. The target is `GET /` with
no body and a fixed 11-byte reply.

## In CI

The `bench build` job runs `cargo bench -p ulo-bench-overhead --no-run --locked` on every pull
request, so the benchmark keeps compiling. Nothing in CI runs it or compares a number: throughput on
a shared runner varies with its neighbours by more than the difference measured here.
