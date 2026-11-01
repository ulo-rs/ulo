# Divergences: race 2b, tests batch 39, NATS roots for every link (F423), a refusal for two crypto providers (F422), the TLS tests on the loader seam, and a probe hook that orders a WebSocket answer (F420)

The sixty-fourth response, signed off 2026-10-17, answers `race2b-tests38.md`. Its S1 probe came
first: against a plaintext NATS server, `add_root_certificates` on a `nats://` url leaves the
connection plain, and a plain connection never reads the file it names. F423 is fixed: every NATS
link, `nats://` included, loads its roots once per link off the worker and hands the file to every
connection, so a `nats://` connection the server upgrades reads the link's file on the blocking
pool. On a `nats://` url to a server requiring TLS, the one-thread scenario holds the thread to 4.6
to 8.0 ms of 25, and a server and a client on `nats://` to a plaintext server stay plain. F422 is
fixed for `tls://`: the link runs rustls's own provider resolution, `ClientConfig::builder()`,
inside `catch_unwind` on the blocking pool, and in a build linking both providers with neither
installed a server's `bind` and every client call are refused naming
`CryptoProvider::install_default`; with the `catch_unwind` removed the refusal reads "loading them
panicked" and both tests fail. The NATS TLS tests now take their roots from the loader seam MQTT's
tests use, the test certificate behind a 50 ms block, and the planted per-connection loads still
fail them, at 110.5 and 111.0 ms and at 2,779.2 ms while reconnecting. F420 has a deterministic
test: `ulo-ws`'s `conformance-probe` gains `AnswerHold`, a wait before an answer is queued, and a
gateway with `max_inflight = 1` holding its first answer passed 200 of 200 runs with the fix and
failed 200 of 200 with the place given back before the queueing. Kafka's `client_producer` is the
slot the client's `connect` reads and stays as it is. `max_inflight`'s documentation states the
order it gives. Two gaps are new: F424, a `ulo-rpc` unit test whose 500 µs deadline can pass while
its client connects, which failed once in the workspace test, and F425, a server link bound through an
app never dropped once the app has closed, which leaves a NATS server's roots file in the temp
directory after every clean shutdown.

Files changed: for S1, S2, S3 and S7, `crates/ulo-rpc-nats/src/link.rs`, `src/lib.rs` and
`Cargo.toml`, a new test file `crates/ulo-rpc-nats/tests/two_providers.rs`, and `Misconfigured`'s
doc in `crates/ulo-rpc/src/link.rs`; for S5, `crates/ulo-ws/src/table.rs`, `src/connection.rs`,
`src/lib.rs`, `Cargo.toml` and `tests/table.rs`; for S8, `crates/ulo-ws/src/gateway.rs`,
`src/module.rs`, `src/table.rs`, `src/connection.rs` and `crates/ulo-ws-hyper/src/lib.rs`.
`Cargo.lock` gains two dependency lines, `rustls` under `ulo-rpc-nats` and `ulo-ws` under `ulo-ws`,
and no package. In the workspace's `FRAMEWORK_GAPS.md`, fix notes under F423, F422, F420 and F417,
their commit written `<commit>`, and F424 and F425 new. The batch started from `14ae28fc` and made no commit. Versions are unchanged at `0.1.0-alpha.1`.

## The signatures

```rust
// crates/ulo-rpc-nats/src/link.rs, crate-private
pub(crate) struct Roots {
    pub(crate) load: fn() -> CertificateResult,
    pub(crate) dir: fn() -> PathBuf,
    pub(crate) resolve: fn() -> Result<(), String>,                    // new: `resolve_provider`
    warned: AtomicBool,                                                  // new
    written: OnceCell<Result<Arc<RootsFile>, String>>,                   // the error is now the whole message
}
impl Roots { async fn load(&self, runtime: &Tokio, tls: bool) -> Result<Arc<RootsFile>, BoxError>; }  // gains `tls`
fn resolve_provider() -> Result<(), String>;                             // new
const PROVIDERS: &str;                                                   // new: the refusal's text

// crates/ulo-ws, under `conformance-probe`
pub struct AnswerHold;                                                   // `pub` in every build; unexported and empty without the feature
impl AnswerHold { pub fn new(hold: impl Fn(&str) -> BoxFuture<'static, ()> + Send + Sync + 'static) -> AnswerHold; }
impl GatewayDefaults { pub fn answer_hold(self, hold: AnswerHold) -> Self; }
// crate-private: `AnswerHold::before_queueing(&self, event: &str)`, awaited in every build;
// `Tracker::new` and `Flight::new` gain the hold.
```

What changes in meaning without a signature:

- A `nats://` NATS link loads the platform's roots once, a server's in `prepare` and a client's at
  its first connect, and writes `ulo-rpc-nats-roots-<pid>-<n>.pem`, where it wrote nothing. A load
  that fails on such a link is logged at `warn` once and the link connects without the file; on a
  `tls://` link it is refused as before.
- A `tls://` NATS link in a process linking both of rustls's providers and installing none fails a
  server's `bind` as `StartupError::Configure` and each client call `Internal` with
  `reason: "misconfigured_link"`, "this process links more than one rustls crypto provider and
  installs none; install one with `CryptoProvider::install_default` before the app starts", where
  its first connection panicked in the caller's await. Where rustls's features name one provider,
  the resolution has rustls install it at the link's first load, where async-nats's first TLS
  connection installed it.
- Every `ulo-rpc-nats` test build links `aws-lc-rs` beside `ring`; its unit tests install `ring`.
- Every `ulo-ws` test build has `conformance-probe` on, through the crate's dev-dependency on itself.

## Decisions

Line numbers are those of the batch's tree unless a line says otherwise.

### 1. The probe: a `nats://` url handed roots stays plain

Read in async-nats 0.46.0: `add_root_certificates` sets `certificates` alone (`src/options.rs:533-536`),
and a connection upgrades when `tls_first` is set, or when `tls_required`, the server's `INFO` or the
scheme requires it (`src/connector.rs:469-509`); `config_tls`, which reads the files, runs only on an
upgrade. Probed against a plaintext `nats:2.10.14` started with `-m 8222`, through a relay of the
probe's own recording the first bytes a client writes after the server's `INFO`
(`probes/p01-s1-root-certificates-on-a-plain-url-0.txt`, source in `probes/src/probe_s1.rs`):

- `nats://` with a readable roots file connected, round-tripped a message, read `tls_required = false`,
  wrote `CONNECT {"verbos..` in the clear, and the server's `/connz` named no `tls_version`.
- `nats://` with a roots file that does not exist connected the same way and wrote `CONNECT`, so a
  plain connection never reads the file.
- The control, `tls://` with the roots file to the same server, wrote a TLS record,
  `16 03 01 00 d5 01`, and failed "received corrupt message of type InvalidContentType", so the relay
  tells the two apart.

The first run of the probe failed to compile on a field async-nats's `ServerInfo` does not have; its
output is kept as `probes/superseded-p01-compile-error.txt`.

### 2. Roots for every link

`Nats::roots` (`crates/ulo-rpc-nats/src/link.rs:151`) loads for every url, at the points it loaded
for `tls://`: a server's `prepare` and a client's first connect, the load and the write on the link's
runtime's `spawn_blocking` inside the `OnceCell`, as batch 38 built them. What a failed load does
depends on the url, which the response left open (batch S1): a url naming a `tls://` server is refused as
before (`:155`); for one naming none, `Roots::warned` lets one `warn` through and the link connects
without the file, async-nats then reading the store itself if a server upgrades the connection. A
plain server needs none of the roots, and a host with no trust store, a `scratch` image among them,
would otherwise have its plain links refused. The refusal's text is unchanged, "the NATS link could not
load the platform's root certificates for `tls://`: ..": it reaches a caller only on a `tls://` url.

### 3. The provider's resolution

The panicking entry is rustls 0.23.45's crate-private
`CryptoProvider::get_default_or_install_from_crate_features` (`src/crypto/mod.rs:243-255`, the
`expect` at `:249`), which `ClientConfig::builder()` reaches through `builder_with_protocol_versions`
(`src/client/client_conn.rs:315`) and which async-nats's `config_tls` calls on every TLS connection
(`src/tls.rs:89`). It takes the installed provider, or installs the one rustls's features name, or
panics when they name two or none. `ClientConfig::builder()` is the public entry, so
`resolve_provider` (`link.rs:994`) calls it inside `std::panic::catch_unwind` and keeps a panic as
`PROVIDERS` (`:982`), the response's text. It runs in the same blocking task, before the store is
read, for a `tls://` url alone (`:951`): on a `nats://` url a successful resolution would have rustls
install a process default for a link that may never use TLS, and a failed one would only be logged
(batch S2). The link installs no provider of its own. Where the features name one, rustls installs it
during the link's first load, which is where async-nats's first TLS connection installed it (batch S3).
The panic hook still prints rustls's message once per refused link. `Roots::resolve` is the seam the
unit tests replace.

### 4. The test for a real panic, and the build that links both providers

A build with both providers and none installed is what makes rustls panic, so `ulo-rpc-nats`
dev-depends on rustls with `aws-lc-rs` (`Cargo.toml`), which no other dependency of the crate's
tests enabled (`probes/p05-feature-trees.txt`: `cargo tree -e features,normal,dev -i rustls`
names the dev-dependency alone, and `-e features,normal` names no `aws`). Every test build of the
crate then links both, as a workspace build already does through the MQTT link. A process with no
installed provider is needed for the panic, and the unit tests' TLS connections need one installed,
so the refusal lives in a binary of its own, `tests/two_providers.rs`, which installs none, and the
unit tests install `ring` through `installed()` (batch S4). Each of its tests first asserts that no
provider is installed and that `ClientConfig::builder()` panics, so a build that stopped linking
both would fail there rather than pass on silence. `cargo test -p ulo-rpc-nats --test
two_providers` needs nothing else.

### 5. The NATS TLS tests on the loader seam (response S3)

`over_tls::slow_test_roots<SLOT>` returns the test certificate as the store's one root after
blocking `STALL * 2`, 50 ms, and counts its loads in `SLOT` of `TLS_LOADS`, MQTT's loader
(decision 137). `TlsNats` gains a `PLAIN_URL` parameter, `false` by default, for F423's scenario on
`nats://` urls. The failure-path unit tests already ran on the seam, an unreadable store being
rustls-native-certs reading a missing file and an empty one a `CertificateResult::default()`, and
read nothing of the host's store; they are unchanged. Three runs (`probes/p02-*.txt`):

- `connecting_over_tls_does_not_stall_the_thread`: 4.6 to 6.1 ms, 64 to 68 ticks, four loads.
- `connecting_on_a_nats_url_to_a_server_requiring_tls_does_not_stall_the_thread`: 4.6 to 8.0 ms,
  63 to 66 ticks, four loads.
- `reconnecting_over_tls_does_not_stall_the_thread`: 2.9 to 7.4 ms in every phase, two loads, so the
  reconnect reused the roots. Two ticks ended while reconnecting in each run, where batch 38 saw 4 to
  138: the file is one certificate, not the host's 158, and the reconnect is quicker (response S8).

The planted loads, batch 38's two (`broken/b01-*.txt`, `broken/b02-*.txt`): with a `ClientConfig`
handed to async-nats, which then reads the platform's store on every connection, the `tls://`
scenario fails at 110.5 ms and the reconnect test at 111.0 ms, both while binding the server, and the
`nats://` scenario at 4,556.9 ms; with the store read on every reconnect alone, through a
`reconnect_delay_callback` skipping each connection's first attempt, the scenarios pass and the
reconnect test fails at 2,779.2 ms while reconnecting. Those breaks read the host's store, being
async-nats's own read; the seam's counts fail without it, with the roots not kept the TLS tests
counting five and four loads where they expect four and two (`broken/b08-*.txt`).

### 6. The plain url stays plain, at the link

`plain::a_nats_url_to_a_plain_server_stays_plain` (`link.rs:1429`), behind `integration`, starts a
plaintext `nats:2.10.14` with its monitoring port, binds a server's link and connects a client's on
`nats://`, each from the test-certificate loader, sends a request that reaches the server, and
requires both links to have written their roots and `/connz` to report two connections and no
`tls_version`. With `require_tls(true)` set where the file is handed, the client's connect fails
(`broken/b07-*.txt`).

### 7. S2, the leftover file

The link's documentation (`Nats`, `link.rs:75`) says a process killed before the drop leaves the
roots file in the temp directory, the roots being public data and the name unique, so a leftover
costs only space; `RootsFile`'s doc says the same in one clause.

### 8. F420: the hook and the test

`AnswerHold` (`crates/ulo-ws/src/table.rs:94`) is carried as `MessagesRead` is: a field of
`GatewayDefaults`, handed to the `Tracker` the table builds and from it to each connection's
`Flight`. `message` awaits `flight.answer_hold.before_queueing(&name)` after the handler's outcome and
before the `match` that pushes it (`crates/ulo-ws/src/connection.rs:1418`), so every answer, an error,
a `complete` and a stream's start included, passes the hold. Under the feature the hold is an
optional `Arc<HoldFn>`, `HoldFn` being `dyn Fn(&str) -> BoxFuture<'static, ()> + Send + Sync`, exported with `GatewayDefaults::answer_hold`; without it
the type is an empty struct and `before_queueing` returns at once. `ulo-ws` dev-depends on itself
with the feature, decision 105's form. An application at `ulo-ws`'s defaults fails to compile a use
of either item, `cannot find AnswerHold in ulo_ws` and `no method named answer_hold`
(`probes/p04-ws-consumer-at-defaults.txt`), and `cargo tree -p ulo-ws -e features,normal` names no
`conformance-probe` where `-e features,normal,dev` names it (`probes/p05-feature-trees.txt`).

`under_a_bound_of_one_a_held_answer_is_not_overtaken` (`crates/ulo-ws/tests/table.rs:621`) serves a
gateway, `/serial`, with `max_inflight = 1` through the table harness on a current-thread runtime.
The client sends `first` and `second` at once; the hold keeps `first`'s answer until `second`'s
answer is about to be queued, or for 300 ms. The test requires the answers in order and the hold's
record to read `first` held until the hold ran out, then `second`. The table harness was chosen over
the conformance suite because it builds its own `GatewayDefaults`.

- With the fix the place is held until the answer is queued, `second` is not read during the hold,
  and the test passed 200 of 200 runs of the built binary (`probes/p03-*.txt`).
- With `handling.done()` back before the hold, the order before batch 38's fix, the read loop reads
  `second` during the hold, its answer is queued first, and the test failed 200 of 200, left
  `[{"id":2,"data":"second"}, {"id":1,"data":"first"}]` (`broken/b09-*.txt`).

### 9. S6: Kafka's `client_producer`

`Kafka::client_producer` (`crates/ulo-rpc-kafka/src/link.rs:98`) is the slot the client's `connect`
reads at runtime: `connect` copies it (`:293`) and makes the client's producer through it (`:308`),
and there is no other path to that producer. `Kafka::brokers` sets it to `producer` (`:126`), and
the one other write is the failed-connect test's (`:1682`). The server's `listen` calls `producer`
directly (`:262`). The response's condition holds and the field is left as it is (batch S5).

### 10. S8: the order `max_inflight` gives

The sentence "Answers to different messages, a stream's items among them, may interleave, each
carrying its message's `id`; only unary answers (one frame, a `complete` or an error) under a bound
of one come back in the order their messages arrived" is on `GatewayDefaults::max_inflight`,
`GatewaySettings::max_inflight` (a new field doc), `WsModule::max_inflight` and
`ulo_ws_hyper::Server::max_inflight`. `Handling`'s doc in `connection.rs` says why a stream's items
are not held to that order: a message holding its place until its stream ended would let a client
with `max_inflight` open streams stop being read, its `cancel` frames among what goes unread. No code
changed for S8.

### 11. S9

Accepted; no action.

### 12. Roots files left behind (F425)

The temp directory held 170 roots files from this session's test runs before the last count. Counted
per test target (`probes/p07-leftover-roots-files.txt`): none for the NATS unit tests,
`two_providers` or the reconnect test, one for each run of the connecting scenario over TLS, and 51
for one run of the NATS conformance suite. A trace of every file written, handed and removed
(`probes/p08-which-roots-file-stays-0.txt`) put the scenario's one on its server's link. A server's
link bound through an app, served, closed, its `serve` returned, every `AppHandle` dropped and the
runtime ended: its `ServerSide` dropped at the close and the `Nats` link never did
(`probes/p09-*.txt`, `probes/p10-*.txt`), so its file stayed. A runtime's tasks drop with it, so what
keeps the link is outside them, likely a cycle through the app's bound server, unread. The link removes
the file when it drops, as decision 7's sentence says; the app does not drop it. Filed as F425, not
fixed: the cause is in `ulo`'s app or `ulo-rpc`'s server, outside the NATS link.

## The tests

- **F423, in `crates/ulo-rpc-nats/src/link.rs`'s unit tests:** `a_plain_link_writes_its_roots_too`,
  which replaces `a_plain_link_writes_no_roots`, and `a_plain_link_whose_roots_did_not_load_is_not_refused`;
  behind `integration`, `over_tls::connecting_on_a_nats_url_to_a_server_requiring_tls_does_not_stall_the_thread`
  and `plain::a_nats_url_to_a_plain_server_stays_plain`, decisions 2, 5 and 6.
- **F422:** `crates/ulo-rpc-nats/tests/two_providers.rs`, `a_tls_server_is_refused_at_bind_naming_the_fix`,
  `every_call_of_a_tls_client_fails_misconfigured_naming_the_fix` and
  `a_plain_link_is_not_refused_for_its_providers`; in the unit tests,
  `a_tls_link_whose_provider_does_not_resolve_is_refused_naming_the_fix` and
  `a_plain_link_resolves_no_provider`, decisions 3 and 4.
- **Response S3:** `over_tls::connecting_over_tls_does_not_stall_the_thread` and
  `over_tls::reconnecting_over_tls_does_not_stall_the_thread` on the seam, decision 5. The unit tests
  install `ring` through `link()`.
- **F420:** `crates/ulo-ws/tests/table.rs`, `under_a_bound_of_one_a_held_answer_is_not_overtaken`,
  decision 8.

### Before and after

The probes and breaks went through `batch39/scripts/brk.py`, batch 38's script, run one spec at a
time by `batch39/scripts/breaks.sh`, which records the container list before and after each; p01 was
a test file copied into `crates/ulo-rpc-nats/tests/` for the run and removed after it, from
`batch39/probes/src/`, and p03 and b09 ran the F420 test 200 times through `batch39/scripts/ws_loop.sh`,
which builds the test binary once and runs that test alone. `brk.py` replaces each span, asserting it
occurs once, runs the targets under bash, writes every touched file and `Cargo.lock` back byte for
byte, and compares a SHA-256 over `git diff HEAD` of `crates`, `Cargo.toml`, `Cargo.lock`, `.github`,
`scripts` and `unsafe-allowed.txt` and every untracked file under `crates` and `scripts`. Specs in
`batch39/specs/`, output in `batch39/probes/` and `batch39/broken/` with `summary.txt` and
`runner.txt`. Every restore reported `ok`, and every container list after a spec matches the one
before it. p01 ran on `14ae28fc` unchanged; p02 and b01 to b08 on `7b601a3d..`, and p03 and b09 on
`7b601a3d..` and again on the final tree, `a5ee435a..`, which differs from it in `ulo-ws` alone (the
type alias under Verification), with the same counts. p06 to p10 ran on `a5ee435a..` after the
verification, p08 to p10 as probe edits through `brk.py`. A first run of b01 to b08, before the
resolver seam was added, is kept in `batch39/superseded/broken-before-the-resolve-seam/`: b05 passed
there, the provider's resolution on a `nats://` url being unobservable in the two-provider binary,
which is what the seam and `a_plain_link_resolves_no_provider` were added for.

| Run | Target | Result |
| --- | --- | --- |
| p01. `add_root_certificates` on `nats://` to a plain server | async-nats through a recording relay | plain, `CONNECT` in the clear, no `tls_version`; a missing file connected too; `tls://`, the control, wrote a TLS record |
| p02. the TLS and plain tests, their numbers printed | NATS's unit tests with `integration`, three runs | 15 passed each; 4.6 to 6.1, 4.6 to 8.0 and 2.9 to 7.4 ms of 25 |
| p03. the F420 test with the fix | 200 runs | 200 passed |
| p04. an application using `AnswerHold` at `ulo-ws`'s defaults | `cargo check` | refused, E0433 and E0599 |
| p05. the feature trees | `cargo tree` | no `conformance-probe` in `ulo-ws`'s normal tree; `aws-lc-rs` from the dev-dependency alone |
| p06. `ulo-rpc`'s deadline test, after it failed in the workspace test | 1,000 runs alone, 200 of the unit binary, 200 beside twelve CPU-bound processes | 0, 1 and 2 failures, each a `Timeout` before the write (F424) |
| p07. roots files left in the temp directory | per test target | 0 for the unit tests, `two_providers` and the reconnect test; 1 per connecting scenario; 51 per NATS suite run |
| p08. every file written, handed and removed | the connecting scenario and the reconnect test | the scenario's server link's file never removed |
| p09, p10. a server's link through an app, closed and dropped | a plaintext server | `ServerSide` dropped at the close; the `Nats` link never dropped and its file stayed (F425) |
| b01. a `ClientConfig` handed to async-nats | NATS's unit tests with `integration` | three TLS tests failed, 4,556.9, 110.5 and 111.0 ms |
| b02. the store read on every reconnect alone | the same | the reconnect test failed at 2,779.2 ms while reconnecting |
| b03. roots for `tls://` urls alone | the same | five failed: the three plain unit tests on their loads, the plain test with no roots written, the `nats://` scenario on `UnknownIssuer` |
| b04. a `nats://` url refused when its roots fail | the unit tests | `a_plain_link_whose_roots_did_not_load_is_not_refused` failed at `prepare` |
| b05. the provider resolved for every url | the unit tests, `two_providers` | `a_plain_link_resolves_no_provider` failed, one resolution; `two_providers` passed |
| b06. the resolution's panic not caught | `two_providers` | both `tls://` tests failed, "loading them panicked: Could not automatically determine.." |
| b07. `require_tls(true)` where the file is handed | NATS's unit tests with `integration` | the plain test failed at the client's connect |
| b08. the roots not kept | the same | 14 failed, among them the TLS tests on five and four loads |
| b09. the place given back before the hold | the F420 test, 200 runs | 200 failed, `second`'s answer first |
| the restored tree | every target above | every test passed |

## Left for the transports DESIGN fold

Line numbers are those of `14ae28fc`.

- §5.3's TLS paragraph (line 1076) and the `Link` sketch's comment (line 994): every NATS link loads
  the platform's roots once per link off the worker and hands the file to every connection, which a
  `nats://` connection reads only once a server upgrades it; on a `nats://` url a failed load is
  logged and the link connects without the file; a `tls://` link in a process linking two rustls
  providers and installing none is refused as `Misconfigured`, naming `CryptoProvider::install_default`.
  A killed process leaves the file in the temp directory.
- §12's table (line 1480): beside the `tls://` NATS row, a process linking two providers and
  installing none; a `nats://` url's failed load is not a refusal.
- Decision 139: its pending clauses settle as decisions 1 to 3 here; the file's lifetime and the two
  direct dependencies are accepted (the response's S2 and S9).
- Decision 140: the tests' loader is the seam, the test certificate behind a 50 ms block, with the
  numbers of decision 5; F423's scenario on `nats://` urls and the plain test beside them; the reconnect
  by the relay is accepted (S4 of batch 38).
- Decision 141: `client_producer` is left as a production field, the slot `connect` reads (decision 9).
- Decision 143 and §4.2's load-shedding paragraph (line 866): the hook, the deterministic test and its
  rate (decision 8), and the order `max_inflight` gives, unary answers under a bound of one, a stream's
  items interleaving by id (decision 10).
- Decision 105: `AnswerHold` and `GatewayDefaults::answer_hold` compile out with the other probes, and
  `fw-ws` dev-depends on itself with the feature.
- A new decision for F422: `resolve_provider`, the test binary, and the dev-dependency on rustls that
  links both providers in the crate's tests (decisions 3 and 4).

## Needs sign-off

Numbered from S1 for this batch.

### S1. A `nats://` link whose roots did not load connects without them

Decision 2. The response said to load roots for every link and did not say what a failed load does on
a url that does not ask for TLS. Refusing it, as `tls://` is refused, would refuse a plain link on a
host whose trust store is empty or unreadable, a `scratch` image among them, where the link works today.
The link logs the cause once and connects without the file; a server that then upgrades the connection
fails its handshake or has async-nats read the store on the polling thread, as before the fix.

### S2. The provider is resolved for `tls://` urls alone; an upgraded `nats://` connection still panics

Decision 3. On a `nats://` url, resolving where rustls's features name one provider would install it as
the process's default for a link that may never use TLS, and an application installing another
afterwards would get `Err` from `install_default`. Not resolving leaves F422 open for one case: a
`nats://` connection a server upgrades, in a build linking two providers with none installed, panics in
async-nats's `ClientConfig::builder()` as before. Resolving and logging, without refusing, was the
alternative.

### S3. The resolution installs rustls's provider at the link's first load

Decision 3. Where rustls's features name one provider and none is installed, `ClientConfig::builder()`
has rustls install it. For a `tls://` link that happens in `prepare` or at the first connect, the same
point async-nats's first TLS connection reached it, so the process ends with the provider it had before.
The panic hook prints rustls's message once per refused link; silencing it would take replacing the
process's hook, global state the application owns.

### S4. Every `ulo-rpc-nats` test build links two providers

Decision 4. The dev-dependency on rustls with `aws-lc-rs` makes the unit tests, the conformance suite
and `two_providers` all two-provider builds. The unit tests install `ring`; the suite's `nats://` links
resolve nothing and are unaffected. A feature enabling an optional rustls dependency would keep the
other test builds to one provider, at the cost of a dependency entry in the published manifest.

### S5. Kafka's `client_producer` stays a production field

Decision 9. It is the one slot the client's `connect` makes its producer through, set to `producer` and
replaced only by the failed-connect test. It was added in batch 38 for that test: before it, `connect`
called `producer` directly, as `listen` still does.

### S6. The NATS suite takes longer

Every `nats://` link in the suite now reads the host's trust store once, on the blocking pool. The
suite took 12.02 s in a run before the verification against batch 38's 5.67 s; the verification's own
time is under Verification. No scenario's bound moved.

### S7. `ulo-ws` dev-depends on itself

Decision 8. Decision 105's form, as `ulo-ws-hyper` and the broker links have it; every `ulo-ws` test
binary is built with `conformance-probe`, so `MessagesRead` counts in the conformance suites too, which
read nothing of it.

### S8. Two ticks while reconnecting

Decision 5. With the file one certificate, the reconnect test's reconnecting phase ends in two gaps per
run, where the host's 158 certificates gave 4 to 138. The test requires at least one, and two is the
margin it now runs on.

## Verification

Full, unfiltered output of every run is in the session scratchpad, `batch39/`: `probes/` for the
probes, with `probes/src/` for the probe test file and `probes/consumer/` for p04's application,
`broken/` and `specs/` for the breaks, `suites/` for the RPC suites, with `summary.txt` and the
container lists before and after each, `verify/` for the checks, `verify/first-pass/` for the first
pass, `superseded/` for runs a later tree replaced, and `scripts/`.

The first pass (`verify/first-pass/`) ran every check below on `7b601a3d..`. Its clippy found one
warning the batch introduced, `type_complexity` on `AnswerHold`'s field (`table.rs:94`), and the field's
type became the alias `HoldFn`, cfg'd with it. The second pass (`verify/`) ran on the final tree,
`a5ee435a..`, read before and after it (`verify/final-tree-hash.txt`, `verify/final-tree-hash-end.txt`),
every check the alias reaches: p03 and b09, the WebSocket tests, the unsafe and runtime-levels checks,
the three `cargo check` runs, the workspace test, the docs and clippy. The RPC suites, the link unit
tests and gRPC's ran in the first pass alone; the alias is in `ulo-ws`, which none of them builds.

- **The RPC suites, once each, all twelve,** one target at a time, `--locked`, libtest's time
  (`suites/summary.txt`): TCP, TCP CBOR and TCP TLS 52 passed and
  3 ignored each (3.26 s each); the smol TCP link's three 52 and 3 each (3.24 s each); UDP 49 and 6
  (3.25 s). With `--features integration --test conformance` and the OpenSSL flags: NATS 54 and 1
  (12.29 s); Redis 53 and 3 (9.15 s); MQTT 59 and 1 (8.23 s); Kafka 55 and 1 (66.28 s); RabbitMQ with
  `ULO_CONFORMANCE_PARALLEL=6` 54 and 1 (28.10 s). Each count is batch 38's. NATS took 12.29 s against
  batch 38's 5.67 s, and 12.02 s in a run before the verification: every `nats://` link now reads the
  host's store once, on the blocking pool (batch S6).
- **The link unit tests with `integration`** (`verify/first-pass/nats-lib-integration.txt`,
  `mqtt-lib-integration.txt`, `kafka-lib-integration.txt`): NATS 15 passed, the eleven unit tests, the
  three over TLS and the plain one, one thread at a time; MQTT 4 and Kafka 4, batch 38's. **NATS's tests
  without it** (`verify/first-pass/nats-tests.txt`): 11 unit, `conformance` 0 (behind `integration`),
  `handle` 2, `two_providers` 3, 1 doctest ignored.
- **UDP's tests** (`verify/first-pass/udp-tests.txt`): `client_address` 3, `client_close` 1, `close_read`
  1, the suite 49 and 6 ignored, `drain_end` 1, `handle` 2, batch 38's.
- **gRPC's tests**: `ulo-codegen-tests`, `builtins` 6, `clients` 8, `compile_fail` 1, `deadlines` 20,
  `enhancers` 7, `grpc` 7, `replies` 12, `stream_end` 6; `ulo-grpc` 1 unit test and 6 doctests ignored;
  batch 38's counts (`verify/first-pass/codegen-tests.txt`, `ulo-grpc-tests.txt`).
- **`ulo-rpc`'s own tests** (`verify/first-pass/ulo-rpc-tests.txt`): 23 unit, `bad_deadline` 7,
  `drain_window` 3, `runtime` 11, `stream_end` 5, doctests 2 passed and 4 ignored, the `compile_fail`
  one; **`ulo-rpc-conformance`'s**: 2 unit, `harness` 7, 2 doctests ignored. Batch 38's counts.
- **The WebSocket tests,** on the final tree: `ulo-ws` (`verify/ws-tests.txt`), `broadcast` 2,
  `conformance`, `conformance_pipe` and `conformance_smol` 31 each, `handoff` 11, `runtime` 1, `table`
  10, one more than batch 38, 4 doctests ignored; `ulo-ws-hyper` (`verify/ws-hyper-tests.txt`),
  `abandoned_handshake` 1, `attributes` 12, `conformance` and `conformance_smol` 31 each, `handshake` 5,
  `hooks` 8, `limits` 7, `messages` 9, `prepare` 2, `runtime` 1, `stream_end` 1; `ulo-ws-conformance` 1
  unit. All passed.
- **The unsafe check,** `python3 scripts/unsafe_allowed.py check`: "self-test: 6 planted lines refused,
  2 valid lines accepted", then 460 files scanned, one more than batch 38, `two_providers.rs`, 2 listed,
  0 findings, exit 0 (`verify/unsafe-check.txt`).
- **The runtime-levels check,** `/usr/bin/python3 scripts/runtime_levels.py --locked`: exit 0, 50
  members, 54 entries, 33 checked, 0 failures, 0 errors (`verify/runtime-levels.txt`).
- `cargo check --workspace --all-targets --all-features --locked` and `cargo check --workspace
  --all-targets --locked`, both with `CFLAGS=-I/opt/homebrew/opt/openssl/include
  LDFLAGS=-L/opt/homebrew/opt/openssl/lib`, and `cargo +1.88 check --workspace --all-targets --exclude
  ulo-http-salvo --exclude ulo-graphql-async-graphql --locked` in `target/msrv-1.88`: exit 0 each, in
  both passes, `ulo`'s 17 known warnings and no location outside `crates/ulo/src`
  (`verify/check-all-features.txt`, `verify/check-defaults.txt`, `verify/check-1.88.txt`).
- **The workspace test,** `cargo test --workspace --no-fail-fast --locked` with the OpenSSL flags. The
  first pass: exit 0, 1,298 passed, 0 failed, 141 ignored, over 202 lines starting `test result:`
  (`verify/first-pass/workspace-test.txt`). Batch 38's run had 1,291 and 141 over 201; the 7 more are
  NATS's three new unit tests, `two_providers`' three, whose binary is the one more line, and `table`'s
  one. The second pass: exit 101, 1,297 passed, 1 failed, 141 ignored, over 202 lines
  (`verify/workspace-test.txt`): `ulo-rpc`'s `client::tests::a_deadline_is_sent_as_milliseconds_the_server_reads`
  failed at `client.rs:1512`, its stream refused `Timeout`, "the call's deadline passed while its client
  connected, so nothing was sent". The batch changed nothing of `ulo-rpc` but `Misconfigured`'s doc, and
  nothing of it between the passes. Probed as p06 rather than rerun: alone it passed 1,000 of 1,000, and
  the unit binary failed it 1 time in 200 and 2 in 200 beside twelve CPU-bound processes, each the same
  `Timeout`; the test's 500 µs deadline can pass while the client connects. Filed as F424.
- `RUSTDOCFLAGS=-Dwarnings cargo doc --no-deps --lib -p <crate> --locked` for `ulo-rpc`, `ulo-rpc-nats`,
  `ulo-ws` and `ulo-ws-hyper`: exit 0 each, in both passes (`verify/doc-*.txt`).
- **Clippy against `HEAD`:** `cargo +1.98.1 clippy -p ulo-rpc -p ulo-rpc-nats -p ulo-rpc-kafka -p ulo-ws
  -p ulo-ws-hyper --all-targets --all-features --no-deps --locked` with the OpenSSL flags, on the final
  tree (`verify/clippy.txt`) and on `14ae28fc` exported with `git archive` into `batch39/head-tree` and
  built in a target directory of its own (`verify/first-pass/clippy-head.txt`): exit 0 each, the same
  warning locations in both, 13 outside `crates/ulo/src`. The batch introduces none.
- **The prose:** a scan for the banned register and for unfilled template braces, asserted first
  against a planted violation (`verify/planted.txt`), over this log, the ledger's new text and every
  added line under `crates`, found none. The scan checks vocabulary only; the text was also read
  against its tests.
- **Containers and images:** at the batch's start twelve containers, the user's, every one exited, and
  25 images (`batch39/docker_start.txt`). Every suite's, probe's and break's container list after it
  matches its list before, and the end's lists match the start's, the same twelve containers by id and
  the same 25 images (`verify/docker-end.txt`, `verify/docker-images-end.txt`). The batch's own
  containers were the suites' and tests' testcontainers, each removed by its scenario, the plain and TLS
  NATS servers among them. The batch pulled no image, ran no `docker ... prune` and started, stopped or
  removed nothing of the user's. Docker answered throughout. p06's twelve `yes` processes were the
  batch's own and were stopped after it. No permission check refused an action.
- **Disk:** 159 GB free when the first pass started, 154 GB at the end (`verify/first-pass/disk-start.txt`,
  `verify/disk-end.txt`), the `HEAD` export's clippy build among the difference.
