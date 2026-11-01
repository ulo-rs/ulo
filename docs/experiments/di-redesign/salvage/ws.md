# Salvage: the WebSocket audit

## 1. Header

**Subject.** The September Smile WebSocket wire audit: RFC 6455 conformance across the five crates that produced the wire at `654c7183` (`ulo-http-axum`, `ulo-http-salvo`, `ulo-http-poem`, `ulo-http-rocket`, `ulo-ws-tungstenite`), plus `ulo-ws-redis`, the broadcast surface and the session store. It filed F196–F215, closed F67, settled D1–D8 on 2026-09-23, and planned 21 PRs in three phases. Rows 1–7 of `BUILD_SEQUENCE.md` (WS-7, WS-9, WS-1, WS-2, WS-3, WS-4, WS-5) merged on `master` as #341–#347 before the pause; the rest never ran. The rebuild replaced every crate the audit read: WebSocket at HEAD is `crates/ulo-ws`, `ulo-ws-hyper`, `ulo-ws-redis`, `ulo-ws-macros`, `ulo-ws-conformance`, designed in `transports/DESIGN.md` §3.5 and §4.

**Source files read in full** (`docs/experiments/di-redesign/salvage/source/ws/`):

| File | Words |
| --- | --- |
| `WS_AUDIT.md` | 6 913 |
| `WS_AUDIT_HANDOFF.md` | 557 |
| `WS_BUILD_HANDOFF.md` | 2 988 |
| `artifacts/ws_wire_probe.rs` | 3 381 |
| `artifacts/spike-d8-transport-bounds.patch` | 601 |
| `artifacts/loops/axum.rs` | 234 |
| `artifacts/loops/salvo.rs` | 295 |
| `artifacts/loops/poem.rs` | 276 |
| `artifacts/loops/rocket.rs` | 276 |
| `artifacts/loops/tungstenite.rs` | 303 |
| **Total** | **15 824** |

Also read: `FRAMEWORK_GAPS.md` entries F197–F200, F202–F207, F209–F213, F215, F66, F67, F190, the F246 mention (line 1338); `BUILD_SEQUENCE.md` §1 (S1, S6–S8, S10), the WS rows and the bookkeeping packets for rows 1–7. F196, F201, F208, F214 and F36 no longer stand in the ledger: rows 1, 4, 6, 7 and 2 deleted them on merge.

**Date** 2026-11-01. **Tip** `331c35b9` (`experiment/di-redesign`). Every HEAD citation is to that tree; no build or run was performed.

## 2. Summary

| Status | Count |
| --- | --- |
| built | 40 |
| built on one path, open on the other (F200, D5) | 2 |
| open | 9 |
| superseded | 21 |
| dropped | 9 |
| UNVERIFIED | 1 |
| **Item rows** | **82** |

Section 5 adds three UNVERIFIED sub-claims attached to built rows (the F212 residual, no Pong after a Close, the unpinned behaviours as a class).

Several items are built in code but pinned by no test; those count as built and carry the missing assertion as their value.

**The five most valuable open items**

1. **D4 / F202 mapping half — the registered close codes were not adopted.** HEAD maps `Unauthorized` and `Forbidden` to `1008` and `Timeout` to `1011` (`crates/ulo-ws/src/gateway.rs:428-439`), where the IANA registry has `3000 Unauthorized`, `3003 Forbidden` and `3008 Timeout`. D4 settled the move on 2026-09-23. Not in the ledger as a HEAD item.
2. **F210 recurs in a new type — the Redis cross-process format is a derived serde shape, unversioned and unpinned.** `Target` and `Audience` derive `Serialize`/`Deserialize` with no attributes, `Audience` is `#[non_exhaustive]` (`crates/ulo-ws/src/broadcast.rs:53-77`), and `ulo-ws-redis` writes `Target` as JSON, a newline, then the frame (`crates/ulo-ws-redis/src/lib.rs:109-119`). No test pins the bytes. A new `Audience` variant changes the wire, and two processes on either side of a rolling deploy drop each other's broadcasts. Not in the ledger.
3. **F207 residual — `Cors` does not reach a same-port upgrade, and nothing says so.** The pre-upgrade decision point exists (pre-dispatch runs before the hand-off, `crates/ulo-http/src/service.rs:51-52, 265-272`; `refuse = handshake` answers 403), so an Origin check is writable. No built-in Origin check exists and `Cors`'s documentation (`crates/ulo-http/src/cors.rs:19-26`) does not mention upgrades. Not in the ledger.
4. **D5 loss — `Connection::close` replaces an invalid code with `1011` and an empty reason, with no log** (`crates/ulo-ws/src/connection.rs:131-141`). That is the clamp D5 rejected: "clamping writes a different frame than the one the handler asked for". `ConnectRefused::code` returns a `Result`, as D5 settled. Not in the ledger.
5. **Six RFC 6455 behaviours the probe measured that no HEAD test pins.** The close echo (§5.5.1 MUST), the `1002` close on a protocol fault, no Pong after a Close, successful fragment reassembly with an interleaved Ping (§5.4), no data after the server's Close, and extension refusal. The code appears to implement each. The conformance client's `hang_up_with` (`crates/ulo-ws-conformance/src/client.rs:284-289`) passes whether or not a Close echo arrives.

## 3. Items

### 3.1 The close frame (spine 1): F196, F197, D1

| ID | What | Status | Evidence at HEAD | Value to extract |
| --- | --- | --- | --- | --- |
| F196 | No Close sent in reply to a peer's Close on salvo, poem, rocket and tungstenite (RFC 6455 §5.5.1 MUST) | built | On a client Close the read loop calls `ws.flush()`, shuts outbound and keeps reading until the stream ends or `close_deadline` passes (`crates/ulo-ws/src/connection.rs:1321-1331`). Nothing asserts the echo: `hang_up_with` reads to EOF either way (`ulo-ws-conformance/src/client.rs:284-289`). | See below. |
| F197 | Every protocol error ends in a bare FIN, no close code (§7.1.7 SHOULD) | built | `read_failure` maps `Protocol` to `1002`, `Utf8` to `1007` and `Capacity` to `1009`, and `send_close` writes the code (`connection.rs:1334-1339`, `:1415-1422`). `ResetWithoutClosingHandshake` stays `Lost` with no frame. `1007` is pinned (`ulo-ws-hyper/tests/hooks.rs:178-183`), `1009` is pinned (conformance `message_limit`, `ulo-ws-conformance/src/cases/messages.rs:82-97`; `ulo-ws-hyper/tests/limits.rs:383-390`). No test sends a frame that should close `1002`. | See below. |
| D1 | Build both: F196 by an extra poll, F197 by a normalised read error mapped to a §7.4.1 code | built | Both halves are in the one loop above. `fw-ws` speaks through `async-tungstenite` directly (DESIGN §4.2 "Tasks and wake-ups"), so the five-library error normalisation D1 costed never arises. | The mechanism, below. |

**F196 value.** The mechanism the audit established is worth keeping because the HEAD fix depends on it. `tungstenite` composes the reply to a peer's Close on receipt and queues it through `set_additional`. The flush runs at the top of the next `read()` (`tungstenite-0.28.0/src/protocol/mod.rs:456-466`, comment *"Since we may get ping or close, we need to reply to the messages even during read"*). `set_additional` is called in four places: two Pong replies (`:508`, `:672`), the close reply (`:737`) and an application close (`:563`). A loop that stops reading after a Close leaves the reply queued. HEAD calls `flush` explicitly and keeps reading, which is the "ask the library for what is wanted" form D8 preferred over relying on the next read. Test idea with its control (`WS_AUDIT.md` §1 and `WS_BUILD_HANDOFF.md` row 7): a raw client sends a Close carrying `1000` + `"bye"` and reads raw bytes to EOF. The expected bytes are `88 05 03 e8 62 79 65`, the echo of code and reason. At `654c7183` axum wrote them and the other four wrote none, through the same reader. A run where every host reports empty has broken the reader. The HEAD suite needs a scenario that requires a Close frame before EOF, since the async-tungstenite client used by the suite would surface it as `Message::Close`.

**F197 value.** The cause differs from F196's: no protocol error queues anything. `tungstenite` returns `Err(Error::Protocol(..))` and `tokio-tungstenite`'s `poll_next` sets `ended = true`, so there is no queued frame to flush. Nothing below the framework composes a close for a fault, because §7.1.7 leaves the code to the endpoint. The probe's five fault cases and their expected codes, with the spec text (`WS_AUDIT.md` §1):

- reserved opcode `0x3`: §5.2 *"If an unknown opcode is received, the receiving endpoint MUST _Fail the WebSocket Connection_"*, code `1002`;
- text frame carrying invalid UTF-8 (`F8 88 80 80 80`): §8.1 MUST fail, §7.4.1 code `1007`;
- unmasked client frame: §5.1 requires the close, the `1002` is a MAY;
- 126-byte Ping: §5.5 caps control payloads at 125, code `1002`;
- frame over the ceiling: code `1009`.

At HEAD the second and fifth are pinned. The first, third and fourth (all `1002`) are not. They are the cheapest missing tests in this subject.

### 3.2 The pre-upgrade decision (spine 2): F198, F199, F190, F205, F207, D2, D3

| ID | What | Status | Evidence at HEAD | Value to extract |
| --- | --- | --- | --- | --- |
| F198 | axum/salvo/poem echo any offered subprotocol and agree to none when offered a list | built | `check_handshake` gathers every `Sec-WebSocket-Protocol` line, splits on `,`, trims, and selects the first of the gateway's declared `subprotocols` the client offered (`crates/ulo-ws/src/connection.rs:787-806`). The conformance `subprotocol` scenario covers two-line offers and no match (`ulo-ws-conformance/src/cases/handshake.rs:217`; DESIGN §4.1 "Subprotocols"). | Spec text below. |
| F199 | rocket and tungstenite negotiate no subprotocol, so the graphql-transport-ws gateway cannot be served there | built | Every host serves through one `GatewayTable::handshake` (DESIGN §4.1 X32). rocket passes 29 of 31 scenarios, the subprotocol scenario included; the standalone port is `ulo-ws-hyper`'s server (DESIGN §4.1 Conformance). | — |
| F190 (adapter half) | A client offering two subprotocols cannot connect | built | As F198. The gateway half belongs to the GraphQL salvage. | — |
| F205 | `WsHandshake::remote_addr` is public and always `None` | built | `ConnectionInfo::peer()` (`crates/ulo-ws/src/transport.rs:290-302`), seeded in the connect phase (`connection.rs:868`). The hand-off passes the HTTP request's `conn.peer` (`handoff.rs:76`). salvo, poem and rocket fill it (`ulo-http-salvo/src/handler.rs:55`, `ulo-http-poem/src/endpoint.rs:48`, `ulo-http-rocket/src/handler.rs:63`). axum fills it only when served through `into_make_service_with_connect_info::<SocketAddr>()` (`ulo-http-axum/src/layer.rs:17-18, 54`). No scenario asserts `Some`. | Test idea: the probe's `who` handler (`ws_wire_probe.rs` P9) reading the peer, asserted `Some` on every host; the axum condition stated in that host's test. |
| F207 | No pre-upgrade point, so the §10.2 Origin check cannot be written; `CorsMiddleware` does not cover the socket | open (residual) | The decision point exists. The unscoped pre-dispatch entries run before the upgrade hand-off (`crates/ulo-http/src/service.rs:51-52, 265-272`), and under `refuse = handshake` a connect guard's refusal answers 403 before the 101 on both ports (`gateway.rs:229-237`; DESIGN §4.1 "Where a refusal happens"). No built-in Origin check exists. `Cors` (`ulo-http/src/cors.rs:19-26`) implements Fetch CORS, which leaves a simple request's origin to the browser, and its documentation says nothing about upgrades. Read, not probed. Not in the ledger as a HEAD item. | See below. |
| D2 | Selection by a declared list on the gateway, intersected with the client's (option a) | built | `#[fw_ws::gateway(subprotocols = [..])]` and `GatewaySettings::subprotocols` (`gateway.rs:121-123, 178-179`). | Rejected options, below. |
| D2 (F211 fold-in) | `event_field() -> &str` becomes `route(&self, msg) -> Option<String>` | superseded | See F211 in §3.5. | — |
| D3.1 | `Dispatched::Upgrade` carries a slot for the negotiated subprotocol | superseded | No `Dispatched`. The handshake decides from `http::request::Parts` and `Switch::response()` writes the 101 with the subprotocol (DESIGN §4.1 X32). The selected name is readable as `UpgradeHead::subprotocol()` (`transport.rs:333-337`). | — |
| D3.2 | The separate-port path uses `accept_hdr_async` | superseded | The separate port is `ulo_ws_hyper::Server`, an HTTP/1.1 server running the same `GatewayTable::handshake` (DESIGN §3.5, second paragraph). | — |
| D3 (non-reversal of ADR-0036) | ADR-0036 decides how a refusal is explained; refusing during the handshake is a separate question | built | Both modes exist: `refuse = close` (default, close code after the 101) and `refuse = handshake` (401/403) (DESIGN §4.1 "Where a refusal happens"; `gateway.rs:229-237`). | The distinction, below. |

**F207 value.** Two arguments from `WS_AUDIT.md` §2 still apply. First, §10.2 asks for prevention and ADR-0036 addressed explanation: *"If the origin indicated is unacceptable to the server, then it SHOULD respond to the WebSocket handshake with a reply containing HTTP 403 Forbidden status code."* A 403 that a browser reports as a bare error still stops the socket from opening, so ADR-0036's objection to a 403 as an explanation is no objection to it as a refusal. Second, the CORS trap: *"An operator who configured an origin allowlist for the HTTP surface on that port has a socket on the same port that the allowlist does not reach, and nothing reports the difference."* At `654c7183` the probe answered `101` to `Origin: https://evil.example` on all five hosts, and on axum with `CorsMiddleware` restricted to `https://good.example` (`ws_wire_probe.rs` P12). HEAD makes the check writable and leaves the trap. Three options: a built-in `Origin` allowlist on the gateway attribute answering 403 under either refusal mode; a sentence in `Cors`'s and `WsModule`'s docs; or both. The probe's P12 is the test, with the allowed origin as its control.

**D2 value.** The two rejected options and why, from `WS_AUDIT.md` §13 D2. (b) Keep echoing and fix only the list split: closes F190's adapter half and leaves the framework agreeing to protocols nothing implements. (c) Never send a subprotocol: uniform and conformant, and makes the GraphQL subscription gateway unserveable on every host. A subprotocol cannot be agreed after the handshake by any mechanism, unlike an Origin refusal, so the selection has to sit before the 101. Spec text the HEAD tests rest on: §4.2.2 *"a single value representing the subprotocol the server is ready to use"*, chosen *"by selecting one of the values from the |Sec-WebSocket-Protocol| field that the server is willing to use"*; §4.3 ABNF `Sec-WebSocket-Protocol-Server = token`; §11.3.4, the multi-line form *"is logically the same as a single |Sec-WebSocket-Protocol| header field that contains all values"*. The probe's four offer shapes (`chat`; `chat, superchat`; two header lines; `no-such-protocol-v99`) are the matrix. HEAD's scenario covers the second, third and a non-match. It does not offer a token the gateway does not list together with one it does; that case decides the preference order.

### 3.3 Close codes: F200, F201, F202, D4, D5

| ID | What | Status | Evidence at HEAD | Value to extract |
| --- | --- | --- | --- | --- |
| F200 | `close_with` puts any `u16` on the wire, `1005` and `7` included | built (connect path); open (message path) | `ConnectRefused::code` returns `Result<Self, CloseCodeError>` and `check_close` refuses codes outside 1000–4999, `1004`/`1005`/`1006`/`1015`, and a reason over 123 bytes (`gateway.rs:336-344, 388-426`). `Connection::close` runs the same check and on failure writes `1011` with an empty reason, with no log (`connection.rs:131-141`). | See D5. |
| F201 | `WsError::Refused` on the message path writes an envelope, not a close | superseded | `WsError::Refused` does not exist. A message handler's failure is the `error` envelope (DESIGN §4.2). A handler closes with `Connection::close`. The connect path writes the Close frame alone (`ulo-ws-conformance/src/cases/close_codes.rs:1-3, 39-43`). | — |
| F202 (comment half) | The `close_code` comment claims RFC 6455 has no auth code | built | DESIGN §4.2 "Close codes" cites the IANA registry and states which codes RFC 6455 defines (1000–1011, 1015) and which are registry entries (1012, 1013). The `ConnectRefused` doc states the mapping without the false claim (`gateway.rs:322-324`). | — |
| F202 (mapping half) / D4 | Map `Unauthorized` to `3000`, `Forbidden` to `3003`, `Timeout` to `3008` | open | `close_code_for` maps `Unauthorized`, `Forbidden`, `BadRequest`, `NotFound`, `Conflict` and `Unprocessable` to `1008`, `TooManyRequests` and `Unavailable` to `1013`, everything else (`Timeout` included) to `1011` (`gateway.rs:428-439`). DESIGN §4.2 and capability [23] (`transports/CAPABILITIES.md:41`, *"policy, rate limit, server fault, or a protocol-specific refusal code"*) carry the old vocabulary. Not in the ledger as a HEAD item. | See below. |
| D5 | Refuse a forbidden code at the constructor with a `Result`, not a clamp | built (connect path); open (message path) | As F200. | See below. |

**D4 value.** The registry facts as fetched on 2026-09-22 from `iana.org/assignments/websocket/websocket.xml` (`WS_AUDIT.md` §3; `FRAMEWORK_GAPS.md` F202): RFC 6455 §11.7 creates the registry and splits it by range, `1000-2999` for *"this protocol and its subsequent versions or extensions"* (Standards Action or Specification Required) and `3000-3999` for *"use by libraries, frameworks, and applications"* (First Come First Served). Entries: `3000 Unauthorized` (2021-09-06), `3003 Forbidden` (2023-02-22), `3008 Timeout` (2024-02-01). `1013` is registered `Try Again Later`, so the `TooManyRequests` mapping is right. The argument for moving: six kinds arrive as one code, so a client cannot tell authentication from validation; the 3000 range is undefined by the protocol and reserved for exactly this choice; the weight is one match arm. The argument for staying: both mappings conform, and a client must be told what a framework's codes mean either way. The lesson recorded in `WS_AUDIT.md` ("Confidence marking"): working from the RFC text alone confirms the wrong comment, because these codes are only in the registry. One coupling: async-graphql's own graphql-transport-ws implementation answers `3008` for the init timeout (`FRAMEWORK_GAPS.md` F187), so the registered code is already on the wire from a library this framework wraps.

**D5 value.** The argument for a `Result` over a clamp (`WS_AUDIT.md` §13 D5): the code is user-supplied, and clamping writes a different frame than the one the handler asked for. Why the forbidden codes matter, with the probe result: a handler returning `close_with(1005, ..)` put `1005` on the wire on all four HTTP hosts at `654c7183`, and a browser receiving `1005` fails the connection, because `1005` is what its own API uses for *"no status code was actually present"*. §7.4.1: *"1005 is a reserved value and MUST NOT be set as a status code in a Close control frame by an endpoint"*, the same for `1006`; §7.4.2: codes `0-999` *"are not used"*. HEAD prevents the forbidden code reaching the wire on both paths. On the message path it substitutes `1011` without telling the caller or the log, so a typo in a code reads at the client as a server fault. Options: `Connection::close` returns `Result<(), CloseCodeError>` as `ConnectRefused::code` does, or the substitution is logged at `warn` naming the code. Test: the probe's `reserved` (1005) and `bogus` (7) handlers (`ws_wire_probe.rs` P8).

### 3.4 Size, extensions, control frames: F203, D6, F209, F66, F67

| ID | What | Status | Evidence at HEAD | Value to extract |
| --- | --- | --- | --- | --- |
| F203 | The ceiling is 16 MiB by inheritance, no knob, no close code | built | `message_limit` on the gateway attribute, `WsModule`, `GatewayDefaults` and `ulo_ws_hyper::Server`; 64 MiB per message at the default and 16 MiB per frame, the frame limit `min(message_limit, 16 MiB)` (`gateway.rs:606-609, 635-638`), applied through `WebSocketConfig` (`connection.rs:714-716`). Breach closes `1009`, pinned. Zero refused in `prepare` (`gateway.rs:846-848`). | See below. |
| D6 | Keep 16 MiB as the default, make it settable, answer `1009` | built | As F203. The settable limit is per message; the 16 MiB is the fixed per-frame cap. | — |
| F209 | No extension is negotiated, and nothing can ask for one | built | Declining is documented: DESIGN §4.2 "Extensions" and the `check_handshake` doc (`connection.rs:759-762`). The capability flag `negotiates_extensions: false` (WS-20) was not adopted. No test offers `permessage-deflate`. | See below. |
| F67 | Ping MUST be answered with a Pong (§5.5.2) | built | Conformance `control_frame` requires the Pong with the Ping's payload as the only frame before the next answer (`ulo-ws-conformance/src/cases/messages.rs:55-78`). The read loop flushes after a Ping (`connection.rs:1315-1319`). | Falsification control: P1's server that completes the handshake and then writes nothing (`ws_wire_probe.rs:324-347`), so a "no Pong" verdict is shown possible. |
| F66 (part 1) | Control frames split three ways between gateway delivery and errors | superseded | Control frames never reach handlers; the protocol layer answers them (DESIGN §4.2, capability [26]; `connection.rs:1315-1320`). | — |
| F66 (part 2) | No keepalive | built | `ping_interval` and `pong_timeout`, 30 s each at the default, a missed Pong ending the connection as `Lost` (DESIGN §4.1 "Keep-alive"); conformance `pings` and `pong_timeout`. | — |
| F66 (part 3) / W2, W3 | A gateway cannot send a Ping or Pong, and cannot observe a Pong | dropped | Deliberate under [26]: the protocol layer owns control frames. | — |

**F203 value.** The bisection result: a 16 777 178-byte payload accepted and 16 777 242 refused on all five hosts at `654c7183` (`ws_wire_probe.rs` P6 used `(1 << 24) - 64`, `1 << 24`, `(1 << 24) + 1024`). The five agreed because they sat on one library. RFC 6455 sets no ceiling, so the number is the framework's to choose once. HEAD's 64 MiB per message is tungstenite's default message ceiling. D6's "keep 16 MiB" referred to the frame boundary the probe measured, which HEAD keeps.

**F209 value.** Why compression cannot sit above the library, read off `tungstenite-0.28.0/src/protocol/mod.rs:641-646`: any frame with a reserved bit set is rejected with `ProtocolError::NonZeroReservedBits`, with no negotiated-extension knob, and `permessage-deflate` sets RSV1 on every compressed message. Upstream state read 2026-09-23 (`FRAMEWORK_GAPS.md` F209): `snapview/tungstenite-rs` issue #2, open since 2017-03-22, 34 comments; PR #144 (2020, closed unmerged, the maintainer objecting that compression is not WebSocket-specific), PR #235 (2021, closed unmerged), PR #426 (2024, open 28 months). Signal ran #426 on a fork in production, withdrew it for unrelated reasons, and does not maintain it. The trigger to revisit is #426 merging. HEAD speaks `async-tungstenite` 0.33 on tungstenite 0.28 for every host (DESIGN §4.2), so one upstream change reaches all of them. Test idea: P11, offering `Sec-WebSocket-Extensions: permessage-deflate` and requiring a 101 without the header, which RFC 6455 §9.1 allows.

### 3.5 Routing, disconnect, session, panic: F204, F206, F211, F212, F214, F246

| ID | What | Status | Evidence at HEAD | Value to extract |
| --- | --- | --- | --- | --- |
| F204 | `DisconnectReason` is always `ClientDisconnect`; the manual-impl workaround reads a constant | built | Six variants, `ClientClose { code, reason }`, `ServerClose { code }`, `ProtocolError`, `Drain`, `Lost`, `RuntimeShutDown` (`gateway.rs:298-320`), passed to `OnDisconnect::on_disconnect` (`gateway.rs:270-272`). Pinned by conformance `on_disconnect_once` (`ClientClose { 4001, "bye" }`), `drain` (`Drain`) and `message_limit` (`ProtocolError`). | — |
| F206 | The handshake query map is a fifth parser and does not percent-decode | superseded | No query map exists. `UpgradeHead` exposes `uri()`, `headers()` and `subprotocol()` (`transport.rs:316-337`); a guard parses the query itself. | If a query accessor is added: the probe URL `/after?room=front%20desk&tag=a%2Bb&flag` must yield `room=front desk`, `tag=a+b`, `flag=` (`ws_wire_probe.rs:962-966`), and S1's crate choice applies (`serde_html_form` 0.4.1, a `serde_urlencoded` fork with sequence fields, over `form_urlencoded`; `BUILD_SEQUENCE.md` S1). |
| F211 | A binary frame can never reach a handler | superseded | `codec = msgpack` routes binary frames through a MessagePack envelope, a frame of the kind the codec does not read closes `1003` (`connection.rs:1438-1470`; pinned, `ulo-ws-hyper/tests/messages.rs:292-296`), and a hand-written `Gateway` receives every data frame raw through `on_message(&self, &Connection, Frame)` (DESIGN §4.1). The proposed `route(&self, msg) -> Option<String>` was not adopted. | A binary protocol that is not MessagePack (protobuf, CBOR, a length-prefixed codec) needs a hand-written gateway and gets no `#[fw_ws::message]` routing. `route()` as a default-bodied override is the recorded alternative that would give attributed gateways that routing (`WS_AUDIT.md` §10). |
| F212 | Panic recovery around the read loop on two of five hosts | built | One loop. A hand-written gateway's `on_message` panic closes `1011` (`connection.rs:1498-1513`); `on_disconnect` is wrapped (`connection.rs:1739`); message handlers run through `fw::dispatch` in the loop's `TaskSet`. | See UNVERIFIED in §5: `run` calls `serve` then `admitted.end` with no drop guard (`connection.rs:727-728`). |
| F214 | `Session::default()` builds a store attached to no connection | built | `Session<T>` and `SessionHandle` have `pub(crate)` fields and no public constructor (`session.rs:18-20, 46-48`); the public `SessionFactory` constructors build factories (`session.rs:123-134`). | — |
| F246 | `WsClient::new(id).session()` reaches the same detached session | dropped | `WsClient` does not exist at HEAD. | — |

### 3.6 Broadcast and the Redis wire: F208, F210, F213, D7

| ID | What | Status | Evidence at HEAD | Value to extract |
| --- | --- | --- | --- | --- |
| F208 | `WsMessage` is not `#[non_exhaustive]`, against reserved opcodes | superseded | The user-facing type is `Frame { Text, Binary }` (`envelope.rs:32-36`), data frames only, not `#[non_exhaustive]`. | Citation correction from the row 6 packet (`BUILD_SEQUENCE.md` line 1197-1202): the reserved opcode ranges are RFC 6455 §5.2 (*"%x3-7 are reserved for further non-control frames"*, *"%xB-F ... further control frames"*), not §11.8, which `WS_AUDIT.md` §8 and F208 cite. Whether `Frame` should be `#[non_exhaustive]` is in §4. |
| F210 (type half) | The broadcast format is `WsMessage`'s derived serde shape | built | The frame crosses processes as JSON `{"event","data"}` and each process re-encodes it per gateway's codec and event field (DESIGN §4.3). | The 4× expansion recorded at `654c7183` (1 KiB binary as 4 108 bytes of JSON) persists in a narrower form that DESIGN §4.3 states: on a MessagePack gateway a byte string arrives as an array of numbers. |
| F210 (pin half) | Nothing pins the cross-process format | open | `Target` and `Audience` derive `Serialize`/`Deserialize` with no serde attributes, and `Audience` is `#[non_exhaustive]` (`broadcast.rs:53-77`). `ulo-ws-redis` writes the `Target` JSON, a `\n`, then the frame bytes (`ulo-ws-redis/src/lib.rs:109-119`). No version field. `lib.rs` holds no `#[test]`; `ulo-ws-redis/tests/` asserts delivery, not bytes. An undecodable message is dropped with a `warn` naming the channel (`lib.rs:281`), an improvement on the old `warn`. Not in the ledger. | See below. |
| F213 | Two broadcast services, no trait, mismatched signatures | built | One `Rooms` binding from `WsModule` and a `BroadcastAdapter` SPI (`prepare`, `publish`, `subscribe`); Redis is `WsModule::for_root().broadcast(..)`, a value and not a second module (DESIGN §4.3). | — |
| D7 (surface) | One async trait, `send(&self, ..)` on both | built | As F213. | — |
| D7 (count) | `send` answers `Result<(), BroadcastError>`, no delivery count | built | `Broadcast::emit` answers `Result<(), BroadcastError>`; `Connection::send` the same (DESIGN §4.3; `connection.rs:120-128`). | The argument: Redis's `Ok(0)` was a literal falsehood and core's `Ok(n)` counted channel sends, not frames delivered; "a number neither can honour is worse than no number" (`WS_AUDIT.md` §9). |
| D7 (wire) | `ulo-ws-redis` gets its own wire type, explicitly tagged, bytes for binary, versioned | open (partly built) | Own type: yes. Explicit tag: no, externally tagged by derive. Version field: no. See F210 pin half. | See below. |
| D7 (membership) | Membership operations async on both | built | `conn.join(..).await`, `conn.leave(..).await` (DESIGN §4.3). | — |

**F210 / D7 wire value.** The rolling-deploy argument transfers to `Target` unchanged (`WS_AUDIT.md` §8, §13 D7): *"A wire format and an in-process enum have different reasons to change, and one derive currently ties them together."* `Audience` being `#[non_exhaustive]` signals that variants will be added, and each addition is a silent cross-process format change: during a rolling deploy, an old process fails to decode a new variant and drops the broadcast. The two remedies D7 settled: pin the literal bytes per audience kind in a test (master's #345 did this for `WsMessage`, one literal per variant plus an unknown-variant case), and give the wire an explicit tag and a version field so a reader can tell "newer than me" from "corrupt". Test idea: encode `Target` for `All`, `Room("lobby")`, `Client(id)` with and without `except`, assert the literal JSON, and assert that a newer audience name decodes to a logged drop rather than a panic.

### 3.7 The connection loop (spine 3): F215, D8, the spike, the loops

| ID | What | Status | Evidence at HEAD | Value to extract |
| --- | --- | --- | --- | --- |
| F215 | `run_ws_connection` written five times, three within the collapse band | built | One loop, `serve` in `crates/ulo-ws/src/connection.rs` (the read loop's eight-branch `poll_fn`, DESIGN §4.2 "Tasks and wake-ups"). Hosts hand `fw-ws` an upgraded `futures-io` stream (`Switch::serve`, DESIGN §4.1 X32). | The measurement, below. |
| F215 (sink) | `TokioSender` copied five times, four `pub` | built | No `TokioSender` in the workspace (grep over `crates/` returns nothing). | — |
| F215 (cardinality) | `MessageCallbackResult::{Continue, Stop, Stream}` beside `Items::{Empty, One, Many}` | built | `fw_ws::Reply = None \| One(Frame) \| Many(BoxStream<..>)` (DESIGN §4.1 `Reply`). | The conflation the audit named: one enum meant both "keep reading" and "here is a task for you to own" (`WS_AUDIT.md` §11). |
| F215 (spawns) | Core spawns nothing; adapters spawn 16 times with abort bookkeeping | built | Zero `tokio::spawn` in `crates/ulo-ws/src`; every task runs on the app's `Runtime` under the table's tracker (DESIGN §4.2 "Tasks and wake-ups"). | — |
| D8 | Core owns the connection loop, the adapter supplies split halves and conversions | built | Core owns it. The shape differs: no adapter-supplied transport; hosts hand over raw upgraded I/O and `fw-ws` runs `async-tungstenite` itself (DESIGN §3.5, §4.1 X32). | The "fix rather than behaviour relied on" argument, below. |
| D8 (ADR) | The D8 ADR, provisional 0062 | superseded | `transports/DESIGN.md` §3.5 and §4 record the design. | — |
| D8 (co-design) | Design `Dispatched::Upgrade(WsUpgrade)` with the HTTP build, not after it | superseded | No `Dispatched` enum; the HTTP server hands upgrades to `UpgradeHandler` (`crates/ulo-http/src/service.rs:265-272`; DESIGN §3.5). | The argument "breaks one enum twice in a single release" (`WS_AUDIT.md` §13 D8) applies to any shared SPI type two builds change. |
| Spike | `spike-d8-transport-bounds.patch`: `Send + 'static` over `SplitStream<T>`/`SplitSink<T, M>` for five socket types | superseded | No split halves cross an SPI at HEAD; the bound `fw-ws` needs is on `Io` (`connection.rs:700-701`). | See below. |
| `loops/*.rs` | The five `run_ws_connection` copies, kept for the measurement | superseded | — | See below. |

**F215 value.** The measurement and its method (`WS_AUDIT.md` §11; `AUDIT3_DISPATCH.md` §4): normalise whitespace, substitute the adapter noun and its library's `Message` type, split into units, count differing units. Results: poem vs rocket 1 unit (the stream type), salvo vs poem and vs rocket 5, axum vs each 25–27, tungstenite vs each 38–40. Collapse band 0–6, split band 35–45. The `loops/` files show the two divergences that were defects. axum's inbound conversion arm is `Err(_) => {}` (`loops/axum.rs:61`) where the others `break`, which is why axum answered a Close: the error arm looped and the next read flushed the queued reply. axum and tungstenite wrap the read loop in `catch_unwind` (`loops/axum.rs:42-69`, `loops/tungstenite.rs:47-97`); salvo, poem and rocket do not. The general lesson: a cleanup guarantee present in some copies and absent in others records no intended posture.

**D8 value.** The argument that outlives the shape (`WS_AUDIT.md` §13 D8, "What this buys beyond deduplication"): a poll-once-more remedy works because of an undocumented tungstenite internal, while a core that calls the library's own close or flush asks for the behaviour directly. HEAD's explicit `ws.flush()` after a Close (`connection.rs:1322`) is that form.

**Spike value.** The claim no longer applies, and the method stands. The positive result was falsified before it was believed: the same assertion over `std::rc::Rc<()>` fails with E0277, so the pass was not vacuous. A first attempt failed for a harness reason that read like a result: the patch script split type paths on `:`, which collided with `::`, and five crates failed with `expected type, found crate` (`WS_AUDIT.md` §13 D8).

### 3.8 The probe and the conformance suite: F36, `ws_wire_probe.rs`

| ID | What | Status | Evidence at HEAD | Value to extract |
| --- | --- | --- | --- | --- |
| F36 | No `WsAdapter` conformance suite | built | `ulo-ws-conformance`: 31 scenarios in seven groups, ten hosts (two reference, five embed, three on smol), stamped by `ws_conformance_suite!` (DESIGN §4.1 Conformance). | — |
| `ws_wire_probe.rs` | The 1 056-line raw-frame probe, twelve cases | superseded | Promoted on master as #342, then replaced by `ulo-ws-conformance`, whose client is `async-tungstenite` over the host's stream with the upgrade request written by hand (DESIGN §4.1 Conformance). | Case map, below. |
| Probe method | Raw frames, byte-at-a-time response head, masked writes, a frame reader reporting `fin`/`rsv`/`opcode`/payload | superseded | The HEAD suite reads frames through a client library. | The rationale: "a client library answers what it decided the wire meant" (`WS_AUDIT.md` §12). The six unpinned cases below are the ones where a library can hide the answer. |
| §12 trap | `read_until(.., opcode == 0x8)` returns `None` on timeout and on EOF alike | built (lesson applied) | The HEAD suite states "No scenario passes on silence, and one about an absence first waits for a positive signal" (DESIGN §4.1 Conformance). `hang_up_with` reads until EOF without asserting what arrived, which is the same conflation for the close echo. | — |

**Case map, `ws_wire_probe.rs` against HEAD.**

| Case | What it measured | At HEAD |
| --- | --- | --- |
| P1 | Ping answered by a Pong with identical payload; control against a silent server | pinned (`control_frame`) |
| P2 close-echo | Close answered with a Close carrying `1000` + `"bye"` | implemented, unpinned |
| P2 ping-after-close | No Pong after the peer's Close (§5.5.2 *"unless it already received a Close frame"*) | unpinned; whether `flush` after a Ping in the closing state writes a Pong is UNVERIFIED (§5) |
| P3 | Subprotocol over four header shapes plus none | pinned (`subprotocol`) |
| P4 two fragments | A `fin=0` text frame and a `fin=1` continuation reassemble | unpinned as a success; `message_limit` sends two fragments only to exceed the limit |
| P4 ping mid-fragment | A Ping between fragments answered without disturbing the message (§5.4) | unpinned |
| P5 | Reserved opcode, bad UTF-8, unmasked frame, 126-byte Ping | `1007` pinned; the three `1002` cases unpinned |
| P6 | Ceiling by bisection | pinned as `1009` over `message_limit` |
| P7 | Raw bytes after each fault and after a Close | superseded by P2 and P5 |
| P8 | Connect guard refusal; handler close `1005` and `7`; `Refused 4406`; unrouted event; binary frame | connect refusal `1008` pinned; `ConnectRefused::code` pinned (`own_code`); unrouted event pinned (`unknown_event`); binary on a JSON gateway `1003` pinned; `Connection::close` with a forbidden code unpinned (D5) |
| P9 close-then-serve | No data frame after the server's Close (§5.5.1 *"MUST NOT send any more data frames after sending a Close frame"*) | unpinned |
| P9 handshake | `remote_addr`, query decoding, protocol header seen by the handler | peer unpinned; query superseded |
| P10 | `WsMessage` serde shape on the Redis channel | open in the `Target` form (§3.6) |
| P11 | `permessage-deflate` offered, none negotiated | unpinned |
| P12 | Origin with and without `CorsMiddleware` | open (§3.2 F207) |

### 3.9 Build rows, sign-offs and couplings

| ID | What | Status | Evidence at HEAD | Value to extract |
| --- | --- | --- | --- | --- |
| Row 1 / WS-7 | Poll once more after a Close | built | Merged on master #341; HEAD's equivalent `connection.rs:1321-1331`. | — |
| Row 2 / WS-9 | Promote the probe to a conformance suite | built | Merged #342; HEAD `ulo-ws-conformance`. | — |
| Row 3 / WS-1 | Correct the `close_code` comment | built | Merged #343; HEAD DESIGN §4.2. Two docs-site pages repeated the false claim (`websocket/gateways.mdx:226`, `techniques/cors.mdx:72`, row 3 packet). | Whether the docs site was rewritten for the rebuild is outside this subject. |
| Row 4 / WS-2 | `Refused` closes on the message path | superseded | Merged #344; `WsError` gone at HEAD. | — |
| Row 5 / WS-3 | Pin the Redis envelope | open | Merged #345 for `WsMessage`; HEAD's `Target` wire is unpinned (§3.6). | — |
| Row 6 / WS-4 | `#[non_exhaustive]` on `WsMessage` | superseded | Merged #346; HEAD `Frame` (§3.6). | — |
| Row 7 / WS-5 | Drop `Session`'s `Default` | built | Merged #347; HEAD `session.rs:46-48`. | — |
| WS-6 | Number kept free for the `catch_unwind` stop-gap | dropped | Never a row. | — |
| Row 77 / WS-20 | `negotiates_extensions: false` in `AdapterCapabilities` | superseded | Documentation in its place (§3.4 F209). | — |
| Row 82 / WS-21 | Core owns the loop (D8) | built | §3.7. | — |
| Row 89 / WS-13 | Declared subprotocols | built | §3.2. | — |
| Row 90 / WS-8 | Normalised read error to a §7.4.1 code | built | §3.1. | — |
| Row 91 / WS-12 | Frame ceiling, `1009` | built | §3.4. | — |
| Row 92 / WS-15 | Origin as `Respond(403)` | open | §3.2 F207. | — |
| Row 93 / WS-16 | Peer address to the handshake | built | §3.2 F205. | — |
| Row 94 / WS-10 | `close_with` returns `Result` | open (message path) | §3.3 D5. | — |
| Row 95 / WS-11 | `3000`/`3003`/`3008` | open | §3.3 D4. | — |
| Row 96 / WS-14 | `route(&self, msg)` | superseded | §3.5 F211. | — |
| Row 97 / WS-17 | `trait Broadcast` | built | §3.6. | — |
| Row 98 / WS-18 | `BroadcastFrame`, tagged and versioned | open | §3.6. | — |
| Row 99 / WS-19 | Handshake query through the S1 crate | superseded | §3.5 F206. | — |
| Row 100 / EXT-12 (adjacent) | Connection hooks read parameters through `FromContext<WsContext>`; `DisconnectReason` as an extractor | superseded | Hooks are traits taking `&ConnectCx` / `&Connection` + `DisconnectReason` (`gateway.rs:264-279`). The extraction salvage owns the row. | — |
| Row 133 / D3-ii-WS (adjacent) | A message handler still running when the socket closes learns it | UNVERIFIED | DESIGN §2.6 and §4.1 describe cancellation reasons and the `TaskSet` the read loop aborts as the connection ends (`connection.rs:1367-1371`); whether the execution's token fires `Disconnected` before the abort was not read through. The gRPC salvage owns D3. | — |
| S1 | One query parser, `serde_html_form` | dropped | No WS consumer at HEAD (§3.5 F206). | — |
| S6 (a) | ADR-0052 amended so `Upgrade` carries `WsUpgrade` | dropped | No `Dispatched`. | — |
| S7 | WS-9 directly after WS-7, before the cluster | dropped | Sequencing for a build that did not run. | — |
| S8 | `HTTP-3` and `WS-21` as two PRs, one permitted | dropped | As S7. | — |
| S10 | One capability record covering `negotiates_extensions` | superseded | HEAD declares host capability as `upgrades: bool` on `EmbedLimits`/backends (DESIGN §3.5; `ulo-http/src/backend.rs:78`, `embed.rs:180`); extensions are documented, not declared. | — |
| Couplings | F201 blocks GQL-6; F206 is the fifth query parser; D1 and F204 widen one callback; F198/F199 are F190's adapter half; D3 consumes the HTTP build; F36 by promoting the probe | dropped | Each names a sequencing dependency between rows that no longer exist. | — |
| Handoff process | Divergence gate, the Law, per-PR checklist, bookkeeping, one `cargo test` at a time (probe binds `19771`) | dropped | Process for a build that did not run. | The method rules in `WS_AUDIT_HANDOFF.md` ("Method") are general: fetch RFC 6455 with `curl` and read it by line number, since `WebFetch` truncates long RFCs and reports sections as absent; fetch the IANA registry separately; probe, then falsify. |

## 4. Worth discussing with the designer

1. **Should the WebSocket close codes move to the IANA registry's `3000`/`3003`/`3008`?** D4 settled yes on 2026-09-23; HEAD and capability [23] keep `1008`/`1011`. Recommendation: move `Unauthorized` to `3000`, `Forbidden` to `3003`, `Timeout` to `3008`, keep `1008` for the client-mistake kinds and `1013`/`1011` where they are. It is one match arm (`gateway.rs:428-439`) and the conformance `guard_refusal` scenario. If the 1008 choice stands, the designer should record why the registered codes were declined, since a later reader will raise D4 again.
2. **Should `Connection::close` return a `Result` instead of substituting `1011`?** Recommendation: yes, matching `ConnectRefused::code` and D5's argument. If the method stays infallible, the substitution should log at `warn` naming the refused code.
3. **Should the Redis cross-process format get its own versioned type, or at least a pinned test?** Recommendation: both. The pin is cheap and catches a variant added to `Audience`. A version field is what lets a rolling deploy tell "newer" from "corrupt".
4. **Should the framework ship an Origin check, and should `Cors` say it does not cover upgrades?** Recommendation: document the gap in `Cors` and `WsModule` at minimum. A gateway attribute `origins = [..]` answering 403 under either refusal mode is the cheap version of RFC 6455 §10.2's SHOULD, since the handshake path already answers 401/403.
5. **Should the six unpinned RFC 6455 behaviours become conformance scenarios?** Recommendation: yes, built on raw-frame writes the suite's client already supports (`Message::Frame`, as `message_limit` uses): the close echo with axum-at-`654c7183`'s bytes as the expectation, the three `1002` faults, no Pong after a Close, reassembly with a mid-fragment Ping, no data after the server's Close, and `permessage-deflate` declined. The close echo is a MUST and is currently correct by inspection only.
6. **Should `Frame` be `#[non_exhaustive]`?** RFC 6455 §5.2 reserves `%x3-7` for further non-control frames. tungstenite fails a connection on them, so no new variant can arrive without a library change. Recommendation: judgment call. `#[non_exhaustive]` costs every user `match` a wildcard arm; the alternative is a documented position that `Frame` follows tungstenite's data opcodes.
7. **Is a non-MessagePack binary protocol meant to require a hand-written gateway?** F211's `route(&self, msg) -> Option<String>` would give attributed gateways a routing hook for protobuf or CBOR frames. Recommendation: record the position. The hand-written `Gateway` is a sufficient escape hatch.

## 5. Not verifiable here

- **F212 residual.** `run` awaits `serve` and then `admitted.end` (`connection.rs:727-728`) with no drop guard. A panic inside `serve`'s own code would skip `on_disconnect` and `hub.unregister`, leaving the connection in its rooms; the slot is freed by `Slot`'s `Drop` on unwind. Whether any code path in `serve` can panic, and whether the runtime's task wrapper catches it, needs a read of the full loop and a fault-injection run.
- **No Pong after a Close.** On a Ping in the closing state the loop calls `ws.flush()` (`connection.rs:1315-1319`). Whether tungstenite queues a Pong after it has received a Close decides §5.5.2's *"unless it already received a Close frame"*. Needs the P2 `ping-after-close` case run against HEAD.
- **Row 133 / D3-ii-WS.** Whether a message handler running when the socket closes sees its execution cancelled before the `TaskSet` abort. Needs a run.
- **Every "implemented, unpinned" verdict in §3.8** rests on reading code. The close echo in particular should be run before it is believed, with the old axum bytes `88 05 03 e8 62 79 65` as the expected output.
