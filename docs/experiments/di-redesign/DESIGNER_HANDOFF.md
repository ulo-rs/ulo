# Designer handoff: ulo's redesign

You are taking over as the designer of `ulo`, a Rust application framework. The previous designer
was an outside chat. It is gone, and this document replaces what it knew. It covers:

- what you decide and how your answers are used;
- what the framework is and where it stands;
- the principles the previous designer applied, which the build now relies on;
- the record conventions;
- the open threads, ending with the first set of decisions waiting for you.

You cannot see the repository. When you need the text of a design section, a log, or a piece of
code, ask for it, and it will be pasted to you. The full designs are long, so ask for the section
rather than the file.

## 1. Your role and the loop

The work runs as a loop with four parties:

1. **You (the designer)** decide.
2. **The user** (Ifeanyi, the framework's author) reads your answer and signs it off. They may also
   send a question back to you first.
3. **Claude Code** (a coding agent in the user's terminal) records your answer verbatim, briefs
   build agents on what was signed off, verifies their work, commits, runs CI, and keeps the
   design documents in step with the code.
4. **Build agents** implement each batch. Every change gets a test that fails against a deliberate
   break and passes once the break is restored. The agents run the conformance suites, write a
   batch log, and list every choice your answer didn't settle as numbered sign-off items (S1, S2…).

You receive a block from each batch. It says what was built and what departs from your answer, and
it lists the S-items. Answer each S-item with one of three:

- accepted;
- changed, saying to what;
- a question back.

The previous designer's answers had this shape:

> **S3: keep the stricter reading.** My wording was loose, and your reading matches the intent. The
> level's claim is that tokio arrives *only* through hyper, h2 or tokio-util. A third-party crate
> that depends on tokio directly breaks that claim exactly as one of ours would… The practical
> effect is useful: adding a dependency that brings tokio now forces a deliberate decision…

Lead with the verdict, then the reason. When you change a decision, say what to build precisely
enough to brief an agent: names, signatures, defaults, and what a test must show. When a decision
depends on how an outside system behaves (a broker, a library, a protocol), say what to probe and
what each outcome decides, rather than assuming. Name an alternative only when it is the real
fork.

**Your premises get checked.** Before anything is built, each factual claim in your answer is
checked against the code, the dependency tree or the library's source. Several of the previous
designer's premises were wrong, and the build said so:

- a compile failure's cause, guessed as a borrow held across an await, was an `async fn` future
  that declared no `Send`;
- `async-nats`'s `flush` sends no PING, so "unsubscribe then flush" couldn't confirm a NATS drain;
- Mosquitto 2.0.18 does not hand an unacknowledged shared-subscription message to another member,
  across 60 probe runs;
- a fix assumed data frames could reorder, but only cancels could.

So state a premise as a claim that can be checked, and expect a correction now and then.

## 2. What ulo is, and where it stands

ulo is a dependency-injection-first application framework, in the spirit of NestJS, for Rust. An
application is a graph of modules and providers wired at startup. It serves that graph over
HTTP, WebSocket, RPC (over TCP, UDP, NATS, Redis, MQTT, RabbitMQ or Kafka), gRPC and GraphQL.

The code is on the branch `experiment/di-redesign` of `github.com/ulo-rs/ulo`. That branch is a
rebuild from first principles, and it will replace `master`. All 43 publishable crates are on
crates.io at `0.1.0-alpha.1`, tagged `v0.1.0-alpha.1`. Every crate inherits one workspace version,
and internal dependencies are pinned exactly, so a release is a one-line version bump.

CI runs 14 jobs on every push:

- **check, test, docs;**
- **MSRV:** 1.88 for most crates, 1.89 and 1.92 where async-graphql and salvo need them;
- **macro diagnostics and generated-code lints,** both pinned to rustc 1.98.1;
- **broker integration:** the RPC conformance suite against real NATS, Redis, MQTT, RabbitMQ and
  Kafka containers, plus a two-process Redis broadcast test;
- **smol:** the HTTP, WebSocket and TCP RPC suites on the smol runtime with smol sockets;
- **runtime levels:** every crate held to its level of runtime independence (§3.3);
- **miri:** listed `unsafe` checked under Miri.

### 2.1 The crates, by layer

| Layer | Crates |
| --- | --- |
| Core | `ulo` (DI graph, app lifecycle, `Execution`, `Runtime`/`Timer` traits, errors, redaction), `ulo-macros`, `ulo-handler-codegen` |
| Shared transport SPI | `ulo-transport` (call/reply traits, `Count`, `TaskSet`, `Tracked`, doc-hidden `__private` primitives: the call-ordered queue, `Watch`), `ulo-transport-macros` |
| Hubs (one per protocol; runtime-free) | `ulo-http`, `ulo-rpc`, `ulo-ws`, `ulo-graphql`, `ulo-graphql-http`, `ulo-graphql-ws`, plus `-macros` crates |
| Sockets and TLS (runtime-free) | `ulo-net` (std sockets, endpoints, socket activation, rustls `ServerConfig`) |
| Runtimes | `ulo-runtime-tokio` (`Tokio`, holding a `Handle`), `ulo-runtime-smol` (`Smol`, `block_on`) |
| Server layer (runtime-neutral) | `ulo-server-hyper` (`Serve<L>`, the `Listener` plug point), `ulo-listener-tokio`, `ulo-listener-smol`, `ulo-http-hyper` (`ServerOn<L>`), `ulo-ws-hyper` (`ServerOn<L>`) |
| Embedding adapters | `ulo-http-axum`, `-salvo`, `-poem`, `-rocket`, `-actix` (serving ulo inside another framework's app) |
| RPC links | `ulo-rpc-tcp`, `ulo-rpc-udp`, `ulo-rpc-nats`, `ulo-rpc-redis`, `ulo-rpc-mqtt`, `ulo-rpc-rabbitmq`, `ulo-rpc-kafka` (tokio-based, each holding its own handle); `ulo-rpc-tcp-smol` over the shared `ulo-rpc-tcp-core` |
| gRPC | `ulo-grpc`, `ulo-grpc-macros`, `ulo-build` (codegen over tonic; tokio-only, because tonic is) |
| WebSocket broadcast | `ulo-ws-redis` (cross-process rooms) |
| Tooling | `ulo-cli` (`ulo new`, `ulo dev`) |
| Test-only (`publish = false`) | the conformance suites `ulo-http-conformance`, `ulo-ws-conformance`, `ulo-rpc-conformance`, `ulo-runtime-conformance`; `ulo-test-certs`; `ulo-codegen-tests`; `ulo-macro-lints` |

Names are `ulo-<role>-<library>`: the role comes first, then the library.

### 2.2 How an application runs

`App::builder(Module)` is given a `.runtime(..)` (spawning plus a clock) or a `.timer(..)` (a clock
alone). The phases are:

1. **`wire()`** resolves the graph.
2. **`connect()`** runs the connect-phase hooks.
3. **`listen()`** binds every transport. It refuses (`RuntimeMissing`) an app that binds a
   transport with no runtime.
4. **Serving.**
5. **Draining:** the app stops accepting, transports refuse new work with "the server is shutting
   down", and in-flight work finishes under a deadline.
6. **`close()`.**

Each call, message or request is one `Execution`, with its own cancellation, deadline and
outcome. `on_stream_end` reports the outcome of the reply actually written. A stream discarded
before it is written was never a reply, so it reports nothing.

## 3. The principles the build relies on

These are the rules the previous designer applied again and again. The build treats them as
settled. Apply them, or say plainly when one should change.

### 3.1 Nothing passes on silence

A test that passes on a timeout, a no-op, or an empty run proves nothing, and the same goes for a
CI job. So:

- every scenario asserts an observable answer;
- every check is shown failing against a planted violation before it is trusted;
- a job that runs no tests fails. The Miri job fails if a pure-Rust `unsafe` entry runs no test
  under Miri.

### 3.2 Refuse at the line that made the mistake

The framework doesn't panic at runtime for a configuration error. A mistake is refused where it is
written, or at the first startup phase that can see it:

- at construction, returning a `Result`: `RpcClient::new` returns `Result<_, UnusableLink>`, and
  `.timeout(Bound::After(ZERO))` returns `ZeroTimeout`;
- at `wire()` or `connect()`;
- at `listen()`.

A permanent misconfiguration is never reported as a retryable `Unavailable`.

### 3.3 Runtimes are given, never found, at two levels

The application picks its runtime. Nothing looks one up ambiently. The independence rule has two
levels, and CI enforces both for every member through `runtime-levels.txt`:

- **Runtime-free:** no `tokio` crate anywhere in the dependency tree. This covers the core, the
  shared SPI, the hubs and their plug-point types, `ulo-net`, `ulo-runtime-smol`, the smol TCP
  link, and the runtime, WebSocket and RPC suites.
- **Runtime-neutral:** the crate runs entirely on the app's runtime. tokio may appear only with
  `sync` (plus `io-util` and `bytes` on the HTTP/2 path, which h2 brings), and only through hyper,
  h2 or tokio-util. Any other direct dependent of tokio fails, a third-party crate included. A
  direct tokio-util dependency needs a named allowance with a reason, and an allowance nothing
  uses fails. This covers the hyper server layer and the HTTP suite.

A plug point inherits the level of the layer it plugs into. The hubs' plug points are
runtime-free. `Listener` and `Incoming` belong to the hyper server layer, so they are
runtime-neutral, because their stream has to speak hyper's I/O traits.

### 3.4 One name, one meaning

A word means the same thing on every call shape:

- `.timeout` on `request` and `send_stream` bounds the whole call;
- on `stream` and `duplex`, the gap between frames is `.idle_timeout`, and the whole call is an
  optional `.deadline`. There is no `.timeout` there; writing one is a compile error;
- one refusal phrase, "the server is shutting down", is used for every drain or close refusal, and
  for WebSocket's 1001 close reason.

### 3.5 Hooks are paired, and the drain refuses new work

A connection that ran `on_connect` always gets `on_disconnect`. A capacity refusal happens before
any hook runs, so a refused connection runs neither. A connection phase that hasn't started when
the drain begins counts as new work, and is refused with 1001 and no hook.

### 3.6 Bounded defaults, and backpressure where the broker can hold it

`max_inflight` defaults to 1,024 on every transport, and `Unlimited` is the explicit opt-out.
WebSocket also has 64 per connection, plus a server-wide 1,024. Over the bound:

- **links whose broker can hold work stop pulling:** AMQP through a channel-wide prefetch, Kafka
  by pausing partitions;
- **the others refuse with `RetryAfter`:** TCP, UDP, NATS, Redis and MQTT. Mosquitto drops held
  messages with the session, which the probe showed.

### 3.7 Order is a property, and one copy beats several

A link's frames go out in the order their sends were *called*. One shared queue enforces this
for every link, so the rule can't drift between links. Wherever two copies of a subtle primitive
would exist, one shared copy is required. Copies left behind have drifted every time.

### 3.8 `unsafe` only where listed, and checked where it can be

The workspace lint level is `unsafe_code = "deny"`. A module opts out only if it's on
`unsafe-allowed.txt`, marked `pure-rust` (checked by Miri in CI) or `syscall` (Miri can't execute
it; it rests on its safety comment and its tests). A faster `unsafe` path that Miri can check is
preferred to a slower safe copy. The cost was measured in the case that decided this.

### 3.9 The conformance suites are the public contract

The HTTP, WebSocket, RPC and runtime suites are how any implementation proves itself, ours or a
community one. Hosts and links declare capabilities. A capability is named as a positive
property, and defaults to the strict reading (for example `confirms_drain` defaults to `true`),
so a capability left undeclared fails a test rather than loosening one. `not_applicable` needs a
stated reason, and is never used to make a run pass.

### 3.10 Standards first, then probe

Before the design commits to protocol behaviour, it checks what the specification decides, what
it leaves open, and what it never mentions. Where an outside system's behaviour decides the
design, the build probes that system before committing.

## 4. The records

| Record | What it holds | How to refer to it |
| --- | --- | --- |
| Core design (`docs/experiments/di-redesign/DESIGN.md`) | The core as it stands; principles §0, decisions §14 | "core DESIGN §9.5", "core decision 25" |
| Transports design (`…/transports/DESIGN.md`) | Every transport, the runtime levels (§8), the suites; decisions §13, numbered 1–126 | "transports §5.3", "decision 104" |
| Responses (`…/transports/RESPONSE.md`) | Every designer answer verbatim, numbered (54 so far), each with the user's sign-off note | "the fifty-fourth response" |
| Batch logs (`…/divergences/race2b-testsNN.md`, `runtime-*.md`) | Per batch: signatures, decisions, tests, the fold list, the S-items, verification | "batch 29's S4" |
| The ledger (`FRAMEWORK_GAPS.md`, outside the repo) | Every gap, deferral and found bug, numbered F1…F389, with dated notes when fixed | "F383" |

Two rules of the record:

- The design documents describe the design as it stands, in the present tense, with no history.
  History lives in the responses and the logs.
- Where a build departs from your answer, the build is what the design states. The departure is
  shown to you as an S-item.

## 5. Where the work goes next

The order:

1. **Batch 30 closes the current round.** After it, a batch is opened only for a real defect: a
   suite failure, a probe finding, or a bug found by reading.
2. **Refinement is deferred, not abandoned.** Better names, tighter grammar and polish are listed
   with their reason instead of batched. They resume in full after the switch, with the same
   thoroughness. When you see a refinement, say so and mark it deferred; don't drop it.
3. **Race 3, the integrations:** configuration, health checks, the database modules (SeaORM, Redis,
   Mongo, Sqlx, Diesel, Prisma) and the CLI's scaffolding for the new API. `master` has all of
   these and the branch rebuilds none of them yet (F301). It starts from a design brief, so your
   first work there is the brief.
4. **The switch:** the examples and integration tests move to the new API (F335), the docs site
   is rewritten, and the branch merges over `master` as a beta towards 1.0. The open pull requests
   on `master` are merged into that transition rather than closed.
5. **Then the deferred refinement resumes.**

## 6. Open threads

### 6.1 Deferred by decision, unbuilt

- **The `async-tungstenite` 0.35 / tungstenite 0.30 bump.** A change of its own, with the
  WebSocket suite as its check.
- **F365:** an opt-in for MQTT to hold work at a broker that does redeliver unacknowledged
  shared-subscription messages, such as EMQX or HiveMQ. It would be declared per broker only after
  the drain scenario runs against that broker.
- **F345:** adopting an already-bound listener or datagram socket in every server
  (`Endpoint::listener` and `Endpoint::socket` in `ulo-net`, taken once). The design is agreed in
  outline, but not built.
- **F324:** building an RPC link from a broker client the host application already owns.
- **F315:** gRPC served only on its own port, not on the HTTP server's port and not inside an
  embedding host. It is deferred.
- **F341:** RabbitMQ's local suite is slow on this host, and is owed one run on a quiet container
  engine.

### 6.2 Standing limits, recorded rather than fixed

- **F383:** every hyper-based server carries tokio's `sync` primitives, which is the reason the
  runtime-neutral level exists.
- **F388:** the `syscall` entries in the `unsafe` list aren't Miri-checked.
- **F382:** rocket hands every request on as HTTP/1.1, so an HTTP/1.0 upgrade gets a 101 there.
- **F378:** rocket can't count connections, so its drain scenario isn't applicable.
- **F377:** hyper closes an accepted connection it hasn't yet read from as idle at the drain.
  That is hyper's behaviour, not a bug.
- **F355:** MQTT can lose a request the broker buffered behind a full window. It is narrowed by
  announcing Receive Maximum 65,535.
- **F331:** a NATS drain can't be confirmed, so NATS declares `confirms_drain(false)`.

### 6.3 Older ledger entries without a fix note

These are F300–F323. Some may be stale, so ask Claude Code to check one against the code before
deciding anything on it:

- F300–F304: wiring errors' advice, an enhancer module tier, mount shaping, core test coverage,
  and the integrations' capability brief;
- F305–F310: `ulo dev` on Unix only, socket activation failing on one bad descriptor, TLS client
  certificates, WebSocket's two transports, an actix-http backend, actix refusing gateways;
- F311–F323: same-named services in wiring reports, a lost 401 challenge, zero hook timeouts, the
  vendored `protoc`, gRPC message size, `ulo dev`'s fixed grace, the HTTP suite's embedding 503,
  axum's nest path, the TCP client's TLS and UDP retries, RPC per-call ordering keys, graphql-ws
  enhancers, an unserved `port = own` gateway.

## 7. The first decisions waiting for you: batch 29

Batch 29 built the fifty-fourth response, the previous designer's last answer. That response:

- restored `FuturesIo`'s faster `unsafe` read in a module of its own, checked by Miri;
- required the Miri job to fail when it checks nothing;
- moved the two `pre_exec` sites onto `command-fds`;
- kept streams with no idle timeout by default, but added a client-wide setting;
- asked that the deadline-waiting scenarios be scaled from the suite's `Budget`, to cut the
  RabbitMQ suite's time.

All of it is built, tested and green in CI:

- **`FuturesIo`:** its `unsafe` read is restored in `ulo-server-hyper/src/compat.rs`, with written
  safety arguments, listed `pure-rust`, and Miri runs its socket-free tests. Miri caught a planted
  uninitialised read.
- **The `unsafe` list:** entries are marked `pure-rust` or `syscall`. The Miri step fails if a
  pure-Rust entry runs no test, and it names each `syscall` entry it skips.
- **`command-fds` 0.3.3:** the two `pre_exec` sites use it, after it passed newly written tests
  for the equal-descriptor case. The list now holds two entries: `FuturesIo` and socket
  activation.
- **`.idle_timeout(Bound)`** is on `RpcClient` and `RpcClientModule`, defaulting to `Unbounded`,
  and a per-call setting overrides it. A lost connection fails waiting calls regardless.
- **The deadline-waiting scenarios** scale from `Budget`.

The six decisions it left:

**S1. `FuturesIo` checks a reader's count before `advance`.** One `assert!` that the old code
didn't have. `advance`'s safety argument now rests on a check rather than on a safe trait's
contract, and a reader reporting more than its slice panics in `FuturesIo`, naming both numbers.
The alternative is the old code exactly, whose argument assumed every `futures-io` reader honest.

**S2. An entry's kind is a word on its line, and a `syscall` entry's reason is the rest of the
line.** `pure-rust` takes nothing after it, so a pure-Rust entry's justification stays in the
comment above it. The alternatives are a reason on every entry, or the kind in the comment block,
which the check would then have to parse out of prose.

**S3. An idle timeout of `Bound::Default` means none, and a zero one is refused.** `Default`
reads the agreed default, no idle timeout. `After(Duration::ZERO)` fails as `ZeroTimeout` ("would
time out every stream"), where it is written or at `wire()`. The alternative is to accept a zero:
a client whose streams all end before their first frame, which nothing would report until the
first stream.

**S4. A call cannot turn the client's idle timeout off.** `StreamCall::idle_timeout` takes a
`Duration`, so on a client with an idle timeout, a call can choose a longer or shorter one but not
none. The alternative is `StreamCall::idle_timeout` taking a `Bound`, with `Unbounded` turning it
off for the call. That changes a signature the previous batch set.

**S5. The deadline scenarios scale with `settle`, and container start is most of RabbitMQ's
time.** At the default budget, the scaling removes about 3.6 s of waiting across seven scenarios.
On suites whose scenarios share one broker it shows directly: TCP went from 4.01 s to 2.01 s, NATS
from 9.17 s to 4.88 s. Each RabbitMQ scenario starts a container of its own, and no scenario there
took under 9 s, so the scaling can't show there. Kafka, whose bounds grow with the budget, went
from 45.8 s to 37.6 s. The alternative for Kafka is a cap (`settle.min(..)`), which brings a fixed
number back.

**S6. `ulo dev` hands each child a duplicate of the held socket.** `command-fds` takes the parent
descriptor by value, so each spawn gets a `try_clone`, which the spawned command closes after the
spawn. If a duplicate can't be made because the process is out of descriptors, `ulo dev` reports
it in yellow, and the child starts with `LISTEN_FDS=1` and nothing at descriptor 3, which its
activation refuses rather than adopting whatever is there. That path wasn't probed. The
alternative is to fail the restart, which the spawn hook's callback, returning nothing, can't do.
