# Questions after the Q20 rulings are built and checked (for the designer)

Written for: the designer. Every ruling in the response to `QUESTIONS_20` is built, checked by a second model and committed (`ac498f28` code, `e76cc7bc` docs, `108e406f` CI allowlist), apart from the items below. The workspace gate runs 2,795 tests with no failure; CI on the branch is green. Cite these as Q21-P1 onwards. The release (`0.1.0-alpha.2`, installed from crates.io) comes after the merge to `master`, once the code is there; it is not part of these questions. Detail: `FRAMEWORK_GAPS.md` F508 to F523.

Paste alongside: transports `DESIGN.md` §2.7, §5.2, §5.3; `unsafe-allowed.txt`.

## Rulings needed

**Q21-P1. A test's `unsafe` in the allowlist.** The Q20-P4 test counts `lapin-io-loop` threads by name, and macOS offers no name for another thread of the process except `proc_pidinfo`, so `crates/ulo-rpc-rabbitmq/tests/one_connect.rs` calls it through `libc`. CI's `miri` job refused it: `scripts/unsafe_allowed.py` allows `unsafe` only in the files `unsafe-allowed.txt` lists, whose header says "keep it short: an entry is a module whose `unsafe` has no safe equivalent". The file is now listed as a `syscall` entry (Miri cannot run a Darwin system call; the file builds nothing on Linux, where the count reads `/proc`). Options: keep the entry; or drop the macOS path and count through a counter the link exposes under `cfg(test)` or a doc-hidden probe, which moves the check from the OS to the link's own bookkeeping and no longer sees a thread the link forgot. Recommendation: keep the entry; the point of the test is the OS-level count.

**Q21-P2. NATS's roots wait is timed on the link's runtime (F521).** Q20-P12 removed the fallback to the link's own runtime from `listen`. NATS's `roots_timeout`, the wait for the roots before subscribing, still runs on the link's runtime, as its docs say, not on the app's `Timer`. Other links' equivalents are unchecked. Does Q20-P12 cover it? Recommendation: yes, so that one `listen` runs on one clock; it does not gate the merge.

**Q21-P3. A configuration naming only `acme-tls/1`.** On HTTP, WS and gRPC such a configuration is refused as `TlsError::Alpn`, because the transport's own protocols are missing. That follows Q20-P2 (the list is the transport's protocols plus `acme-tls/1` when named). An ACME library's configuration that names only `acme-tls/1` has to add `h2` and `http/1.1` by hand. Intended, or should `from_config` supply the transport's protocols when the configuration names none of them? Recommendation: keep the refusal; it names what is missing.

## Confirmations

**Q21-P4. The handshake test for WS and gRPC.** Q20-P2 asked that a client offering only `acme-tls/1` completes the handshake on HTTP, WS and gRPC. HTTP has it (`ulo-http-hyper/tests/tls_config.rs`); WS and gRPC are pinned at the offered-list level in `ulo-net/tests/tls.rs`, because neither has a TLS server test file. Accept, or build the two?

**Q21-P5. The NATS bound depends on `retry_on_initial_connect` staying unset.** async-nats 0.46 defaults it to false, and then `connect` awaits the first answer inline, so dropping the future at the bound cancels the connect and closes the socket. If it were set, `connect` would return at once and the bound would neither see nor cancel the connect. Transports §5.3 says so, and a planted break that sets it fails three tests. Accept as a documented dependency.

## For the record

**Q21-P6. What the Q20 round found in the RabbitMQ and NATS work.** No behaviour defect in the code. Two statements of what is bounded were wrong and are fixed: RabbitMQ's `close` uses no clock (only its connect is timed on the `Timer`), and NATS's `flush` is a socket write, not a round trip, so no bound covers the server confirming the subscriptions (async-nats 0.46 `lib.rs` 633-642). One conformance test, `a_call_held_past_a_spent_cap_is_answered_unavailable`, failed once in a full gated RabbitMQ run and not in 26 later runs, the shape F482 and F479 record (F522). Refinements: a panicking connect task now answers a plain error so `is_panic` is false where HEAD re-raised in the caller; two concurrent direct `Link::connect` calls overwrite `state.client`, which the client's lock prevents (F523).

**Q21-P7. Salvage.** The September Smile campaign is now read item by item into `docs/experiments/di-redesign/salvage/`, with 22 cross-subject questions in `salvage/QUESTIONS_SALVAGE.md` and findings F524 to F561 in the ledger. Those questions are the next agenda after these.
