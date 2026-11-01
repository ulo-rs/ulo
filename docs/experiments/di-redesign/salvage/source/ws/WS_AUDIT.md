# The WebSocket audit

The WebSocket wire against RFC 6455, across the five crates that produce it —
`ulo-http-axum`, `ulo-http-salvo`, `ulo-http-poem`, `ulo-http-rocket` and `ulo-ws-tungstenite`.
The brief is `WS_AUDIT_HANDOFF.md`; its scope line names *"subprotocol negotiation
(`Sec-WebSocket-Protocol`), the close-code registry against ulo's 1008/1011/1013 choices,
fragmentation, and payload limits"*, and puts `ulo-ws-redis`, the broadcast surface and the session
store in scope *"because nothing else will take it"*.

Out of scope by the brief: guards, interceptors and the error chain on WebSocket, which the pipeline
audit took across all four transports at once and settled — its build's PR 8 covers a guard's refusal
on both WebSocket phases and PR 10 covers F131. What that pass explicitly left is the wire and the
adapter SPI beneath it, and §11 takes the SPI because no other pass will.

## Where the findings live

`FRAMEWORK_GAPS.md`, **F196–F215**. This pass was invoked as an audit of the framework rather than of
the documentation site, so everything it found files there.

Two existing entries moved. **F67 is closed** — the one open MUST in the framework is met on all five
adapters, established rather than assumed. **F66** gains the probe confirming its first part, and a
correction: the brief recorded W2, W3 and W4 as unfiled, and W2 and W3 are already F66's second and
third parts. Only W4 was unfiled, and it is F208.

This memo does not restate the findings. It says what they add up to and which calls are the user's.

## Not a build

Nothing was changed. No branch exists. The probe file was deleted from the tree and its registration
in `integration-tests/tests/integration/main.rs` restored; `git status` is clean at `654c7183`. The
probe is kept in `artifacts/` beside this memo as `ws_wire_probe.rs`, 1 056 lines, twelve cases — nine
across all five adapters, two across the four that serve a same-port upgrade, and one that needs no
server. Promotable to the conformance suite F36 has been asking for since 2026-09-12. §11's divergence
measurement is in `loops/` beside it.

## Confidence marking

- **Probed** — run this session against master `654c7183` with a raw-frame client that performs the
  HTTP upgrade by hand, writes masked frames, and reads opcodes and bytes off the socket. No client
  library sits between the assertion and the wire.
- **Read** — read off the source, cited by file and line.
- **Spec** — the published document's own words, quoted rather than paraphrased.
- **Unverified** — believed, not established. Marked inline and not acted on.

RFC 6455 was fetched to the scratchpad and read by line number, not recalled. That mattered once:
the close codes this audit recommends reconsidering are **not in RFC 6455**. They are in the IANA
WebSocket Close Code Number Registry, which §11.7 creates and which was fetched separately from
`iana.org/assignments/websocket/websocket.xml`. An audit working from the RFC text alone would have
confirmed the comment in `ws_error.rs` instead of falsifying it.

Every probe carries a control. The Ping case ran against a server that completes the handshake and
then writes nothing, and reported the absence. The close-frame cases have their control inside the
result: axum writes the bytes the other four do not, through the same reader.

## The shape of it

**One mechanism produces the two MUST-class findings, and one missing seam produces the rest.**

The mechanism is that a queued reply is flushed by the **next** read, and four adapters stop reading.
`tungstenite` composes the close reply on receipt and queues it; the flush runs at the top of the next
`read()` loop iteration, not at the point of receipt. Four adapters break out of the read loop when
the peer closes, so the second read never happens and the reply dies queued (F196, a §5.5.1 MUST).
axum converts the Close to an error, ignores it, and loops — which is why the adapter that appears to
discard the Close is the only one that answers it.

F197 looks like the same finding and is not. No protocol error queues anything: `tungstenite` returns
an error and `tokio-tungstenite` marks the stream ended, so there is no queued frame and no flush to
reach. Nothing below ulo composes a close for a protocol fault, by design — §7.1.7 puts the choice of
status code on the endpoint, and ulo is the endpoint. All five adapters answer every protocol error
and every oversize frame with a bare FIN because nothing anywhere writes that frame.

One is a missing poll. The other is a close ulo has never written.

The missing seam is a **pre-upgrade decision point**. `register_ws_route` hands the adapter a path and
three callbacks, and the first of them runs after the `101` has been written. Three separate findings
are that one absence: the subprotocol has to be selected during the handshake and no gateway can
declare what it speaks, so three adapters echo whatever arrives and two echo nothing (F198, F199);
the Origin check RFC 6455 §10.2 recommends answers `403` on the handshake, and ulo has no seam that
can write a status (F207); and the peer address that a connect guard would gate on is never carried
across that boundary (F205). The constraint is a property of the SPI rather than of the protocol.

The remainder are independent and can be read in any order: what a close code may be (F200, F202),
where `WsError::Refused` is honoured (F201), what the size ceiling is and who chose it (F203), what
`DisconnectReason` can ever say (F204), how the handshake query is parsed (F206), and what crosses
the Redis channel (F208, F210). F209 records a capability nothing has.

Underneath both sits a third, which §11 takes: **the connection loop is written five times and three
of the copies are identical by the repository's own measurement rule.** F196 is one match arm that
diverged between copies. F197 is a close frame no copy writes. F204 is a parameter the closure type
cannot carry. F212 is a cleanup guarantee two copies have and three do not. Those are not four
defects that happen to share a subject; they are what a five-times-copied loop produces, and the
answer `HttpAdapter` is already getting applies unchanged.

Sections 1 through 4 below are the first two spines, 5 through 10 the independent findings, and §11
the third spine.

## 1. The close frame ulo never writes — F196, F197

RFC 6455 §5.5.1: *"If an endpoint receives a Close frame and did not previously send a Close frame,
the endpoint MUST send a Close frame in response. (When sending a Close frame in response, the
endpoint typically echos the status code it received.)"*

Probed. A raw client sends `1000` + `"bye"` and reads every byte:

| Adapter | Bytes written before FIN |
| --- | --- |
| axum | `88 05 03 e8 62 79 65` — the echo, code and reason |
| salvo | none |
| poem | none |
| rocket | none |
| tungstenite | none |

All five within 0 ms. The same reader, on the same probe run, is what produced axum's bytes, so the
four empties are an absence rather than a missed read.

§7.1.7 covers the other half: *"the endpoint SHOULD send a Close frame with an appropriate status
code (Section 7.4) before proceeding to _Close the WebSocket Connection_."* Five cases, five
adapters, twenty-five results, all identical — `bytes=[] then FIN`:

- a reserved opcode `0x3`, where §5.2 is *"If an unknown opcode is received, the receiving endpoint MUST _Fail the WebSocket Connection_"* and `1002` is the code
- a text frame carrying invalid UTF-8, where §8.1 is a MUST to fail the connection and §7.4.1 gives `1007`
- an unmasked client frame, where §5.1 requires the close and makes the `1002` a MAY
- a 126-byte Ping, where §5.5 caps a control payload at 125 and §7.4.1 gives `1002`
- a frame over the ceiling, where §7.4.1 gives `1009`

What reaches a client in all twenty-five cases is `1006`, the code §7.4.1 reserves for *"the connection was closed abnormally, e.g., without sending or receiving a Close control frame"*. A clean shutdown, a
protocol fault, an oversize message and a killed process are one event at the peer.

**The two halves have different causes, which the first reading of this probe got wrong.** Read off
`tungstenite-0.28.0/src/protocol/mod.rs`: `set_additional` is called in four places — two Pong
replies (`:508`, `:672`), the close reply to a peer's Close (`:737`), and an application-sent close
(`:563`) — and `read` flushes whatever is queued at the top of each iteration, commented *"Since we
may get ping or close, we need to reply to the messages even during read"* (`:456-466`).

So F196 is a reply that exists and is never flushed, and F197 is a reply that is never composed. The
Ping result confirms the same mechanism from the other side: all five adapters keep reading after a
Ping, all five flush the queued Pong, and F67 closes clean. One fix does not cover both.

## 2. The handshake is over before anything can decide — F198, F199, F205, F207

`HttpAdapter::register_ws_route(path, callbacks)` and `WsAdapter::register_gateway(port, path,
callbacks)` are the whole registration surface, and `WsConnectionCallbacks` holds connect, message
and disconnect. The connect callback runs post-upgrade by construction.

ADR-0036 reached this boundary and stopped at it deliberately. Its context section rejects refusing
during the handshake: *"browsers deliberately withhold the status and body of a failed WebSocket
upgrade, so a `403` reaches a JS caller as a bare error event with nothing in it."* That is correct
and it settles a different question. ADR-0036 decides **how a refusal is explained**, and the three
findings below are about **what can be decided before the socket exists** — which subprotocol is
agreed to, whether the origin is acceptable, and what address the peer came from. None of the three
is answerable by a frame sent afterwards, and the subprotocol is not answerable afterwards at all:
§4.2.2 puts it in the handshake response or nowhere.

**The seam these want is already being built, and this section as first written did not know it.**
`HTTP_AUDIT.md` §1 removes `register_route` and `register_ws_route` and replaces them with one call:

```rust
pub async fn dispatch(&self, req: HttpRequest) -> Dispatched;

#[non_exhaustive]
pub enum Dispatched {
    Respond(HttpResponse),
    Upgrade(Arc<WsConnectionCallbacks>),
}
```

That decision is taken from the ulo `HttpRequest`, before the adapter performs the handshake. It is
the pre-upgrade decision point, it was spiked on poem and salvo on 2026-09-20 and falsified with
`if false && …`, and rocket already ships the shape by hand through a magic response header. Two of
the three findings below become cheap under it rather than needing a seam of their own: **F207** is
`Dispatched::Respond(403)`, and **F205** is an insertion into the request's `extensions` the way
axum's `ConnectInfo` travels. What remains is narrower and is D3.

**Subprotocol.** §4.2.2 defines the value a server sends as *"a single value representing the
subprotocol the server is ready to use"*, chosen *"by selecting one of the values from the
|Sec-WebSocket-Protocol| field that the server is willing to use"*, and §4.3's ABNF is
`Sec-WebSocket-Protocol-Server = token`. Probed, one gateway with no subprotocol of its own:

| Client sends | axum / salvo / poem | rocket / tungstenite |
| --- | --- | --- |
| `chat` | `101`, echoes `chat` | `101`, no header |
| `chat, superchat` | `101`, **no header** | `101`, no header |
| two header lines | `101`, echoes `chat` | `101`, no header |
| `no-such-protocol-v99` | `101`, echoes it | `101`, no header |

Three adapters agree to anything, including a protocol no code in the application implements. The
same three agree to nothing when the client offers a fallback, which is F190's adapter half: the raw
comma-joined string is tested for membership in the parsed list and is not a member. The two-header
form is read as its first line, where §11.3.4 says it *"is logically the same as a single
|Sec-WebSocket-Protocol| header field that contains all values"*.

rocket and tungstenite negotiate nothing at all. That is conformant for a server — null is a legal
`/subprotocol/` — and §4.1 step 6 has a client that required one fail the connection. The consequence
is concrete: ulo's own graphql-transport-ws subscription gateway cannot be served on rocket or on a
separate port.

The common cause is that a `Gateway` cannot declare a subprotocol. With nothing to select from,
echoing is the only behaviour left, and echoing is what three of them do.

**Origin.** §10.2: *"Servers that are not intended to process input from any web page but only for
certain sites SHOULD verify the |Origin| field is an origin they expect. If the origin indicated is
unacceptable to the server, then it SHOULD respond to the WebSocket handshake with a reply containing
HTTP 403 Forbidden status code."* The recommended answer is a status on the handshake. Probed: an
upgrade carrying `Origin: https://evil.example` was answered `101` by all five, and the same upgrade
against an app whose global chain carries `CorsMiddleware` restricted to one origin was also answered
`101`. A `403` a browser reports as a bare error still prevents the socket from opening, which is
the property §10.2 is after — ADR-0036's objection is to a `403` as an explanation, not as a
refusal. Under `Dispatched` the check is a guard-shaped function reading the request and answering
`Respond(403)`, with no new SPI.

The CORS result is correct and is the trap. The Fetch standard does not govern WebSocket, and a
simple request's origin is enforced by the browser rather than by the server, so a `101` is what a
conforming middleware produces. An operator who configured an origin allowlist for the HTTP surface
on that port has a socket on the same port that the allowlist does not reach, and nothing reports the
difference. A connect guard reading `handshake.headers["origin"]` works and refuses with a `1008`
close after the handshake completed, which is not what §10.2 asks for.

**Peer address.** `WsHandshake::remote_addr` is `pub`, documented as *"Remote address"*, and written
`None` in the only two places that construct it. Probed `None` on four adapters. A guard that would
rate-limit by address reads a constant.

## 3. What a close code may be — F200, F202

Two separate questions that meet at `close_code` and `close_with`.

**Codes the protocol forbids.** `close_with` accepts any `u16`. §7.4.1: *"1005 is a reserved value and
MUST NOT be set as a status code in a Close control frame by an endpoint"*, and the same for `1006`;
§7.4.2 says codes in `0-999` *"are not used"*. Probed: a handler returning `close_with(1005, …)` put
`1005` on the wire on all four HTTP adapters, and `close_with(7, …)` put `7`. A browser receiving
`1005` fails the connection, because `1005` is what its own API means by *no status code was
present* — the handler's close reads as a frame that could not have been sent. One check at one
constructor covers every close frame ulo writes.

**Which codes to choose.** This is the question the brief named, and the answer is not in RFC 6455.
§11.7 creates the IANA WebSocket Close Code Number Registry and splits it by range: `1000-2999` for
*"this protocol and its subsequent versions or extensions"*, `3000-3999` for *"use by libraries,
frameworks, and applications"* under First Come First Served. The registry carries `3000 Unauthorized`
(2021-09-06), `3003 Forbidden` (2023-02-22) and `3008 Timeout` (2024-02-01).

ulo maps six `ErrorKind`s to `1008` and everything unmatched to `1011`. The comment above the mapping
states *"RFC 6455 has no auth-specific code, which is why an unauthorized connect is also 1008"*, and
the registry the RFC creates has one. `1013` for `TooManyRequests` is right — it is registered as
`Try Again Later`.

This is the three-category test's second kind: the protocol leaves the meaning of `3000-3999`
undefined and reserves the range for exactly this. The choice is ulo's, has to be made once, and is
currently made by a sentence that is wrong about the alternatives.

## 4. Where `WsError::Refused` is honoured — F201

The variant documents itself as *"Refuse the connection with a close code and nothing else on the
wire"*, and `refusal_frames` implements that. `refusal_frames` runs on the connect path only.

On the message path, `handle_message` renders through `ws_err.to_message()`, where `Refused` falls
into the same arm as every other variant, `kind` is fixed to `"BadRequest"`, and `close_code` is
never consulted. Probed on four adapters: a handler returning `Err(WsError::Refused { code: 4406, …
})` put the canonical envelope on the wire, no close frame, socket left open — the exact inverse of
the doc comment.

The connect path works. Probed, all four: envelope, then `1008` with the reason, then FIN.

This blocks F187's candidate fix. That finding routes all five graphql-transport-ws closures through
this variant, and on the message path the variant carries no code to the peer.

## 5. The ceiling nobody chose — F203

`max_frame_size`, `max_message_size` and `WebSocketConfig` appear in no file under `crates/`.

Probed by bisection: a payload of 16 777 178 bytes is accepted by all five adapters and one of
16 777 242 bytes is refused by all five. The boundary is 16 MiB exactly — `tokio-tungstenite`'s
default `max_frame_size`. The five agree because they sit on one library, not because a number was
picked. RFC 6455 sets no ceiling, which puts this squarely in the class a framework must decide.

The refusal is F197's shape: no `1009`, no close frame, TCP drop. A caller who needs a different
ceiling has nowhere to set one, and the current number changes when the dependency does.

## 6. What `DisconnectReason` can say — F204

`handle_disconnect` takes a `DisconnectReason` and has one caller, which passes the literal
`ClientDisconnect`. `ServerShutdown`, `Timeout` and `Error(String)` are constructed nowhere, and
`DisconnectReason::error` builds a value nothing produces.

The reason cannot arrive from below: `WsConnectionCallbacks::disconnect` takes a client id, and the
stored closure is `dyn Fn(String) -> …`.

`DOCS_AUDIT.md` §17 A6 records that the macro forms do not forward the reason, and names a manual
`Gateway` impl as the route to reading it. A manual impl reads `ClientDisconnect` for every cause.
The workaround on record does not work, which is worth more than the original entry.

## 7. The fifth query parser — F206

`create_client_from_parts` splits the handshake query on `&` and `=` and stores the pieces unchanged.
The four parsers F117 and F180 are about all run `serde_urlencoded::from_str`, which decodes.

Probed on four adapters: `/path?room=front%20desk&tag=a%2Bb&flag` reaches a gateway as
`room=front%20desk`, `tag=a%2Bb`, `flag=`. A connect guard comparing a token against a stored value
fails for any token the client encoded, and fails as a refusal rather than an error.

The README's cross-audit coupling reads *"Four query parsers, three owners."* It is five parsers, and
this is the only one that does not decode at all.

## 8. What crosses the Redis channel — F208, F210

`ulo-ws-redis` publishes `WsMessage` as JSON. The derived representation is externally tagged:
`{"Text":"hi"}`, `{"Binary":[0,1,2,250]}`, `{"Close":{"code":1008,"reason":"nope"}}`, `{"Close":null}`.
Probed: a 1 KiB binary frame serialises to 4 108 bytes, each byte a decimal number in a JSON array.

Nothing in the repository asserts that shape, so a `#[serde(rename)]` or a new variant changes the
cross-process wire format with no test failing. A payload naming an unknown variant is dropped with a
`warn` that names neither the variant nor the channel.

That couples to F208. `WsMessage` carries no `#[non_exhaustive]`, and RFC 6455 §11.8 reserves
opcodes `0x3`-`0x7` and `0xB`-`0xF` for further frame types, so the enum will grow. Adding a variant
is a breaking change for every downstream `match` **and** a silent format change on the Redis
channel, which makes two processes on either side of a rolling deploy drop each other's frames.

## 9. Two services with one job and no trait between them — F213, F214

`grep 'trait Broadcast'` over `crates/` returns nothing, and the two implementations do not agree on
a signature:

| | `BroadcastService` | `RedisBroadcastService` |
| --- | --- | --- |
| `join_room`, `leave_room`, `get_*` | synchronous | `async` |
| `send` receiver | `&self` | `mut self` |
| `send` answer | `Ok(count)` | `Ok(0)`, always |

`RedisBroadcastModule::for_root` exports both types. So the documented path to multi-process — import
`RedisBroadcastModule` in place of `BroadcastModule` — describes a module swap the types do not
support: a gateway written against one does not compile against the other.

Both counts are unreliable and differently so. Redis returns zero because delivery is unknowable at
publish. Core returns the number of sinks it wrote to, which is a count of successful channel sends
rather than of frames delivered. A number neither can honour is worse than no number.

The membership operations sit here rather than out of scope. RFC 6455 has nothing to say about rooms,
so they are not a conformance question, and they are where the sync/async split shows.

Beside them, `Session` — the per-connection typed store, which is the coherent part of this surface —
derives `Default`, so `Session::default()` builds a store attached to no connection (F214). The
private field on `WsClient` exists to stop exactly that state being reached by replacement; the derive
reaches it by construction. `Session::new()` has one caller and `Session::default()` has none.

## 10. A binary frame cannot be routed — F211

`extract_event` answers `WsError::InvalidMessage("Binary messages not yet supported for event
extraction")` for `WsMessage::Binary(_)`. Probed on four adapters: that envelope is what a binary
frame gets.

The framework sends binary and cannot receive it. `WsMessage::binary` is a public constructor,
`Items::One(WsMessage::Binary(..))` is a valid answer, and `BroadcastService::send` takes any
`WsMessage`. What blocks the inbound direction is that routing is fixed to a JSON envelope:
`Gateway::event_field() -> &str` names a string field, which presumes the frame parses as a JSON
object.

This is §2's finding in a different place — **a decision the framework hardcodes where the gateway
should declare it.** The override point already exists and is one level too low: generalising
`event_field() -> &str` to `route(&self, msg: &WsMessage) -> Option<String>` makes binary routable
with no new concept. Two call sites and one override in the workspace, which returns `"type"` for
graphql-ws and gets shorter under the change.

## 11. The connection loop is written five times — F215, F212

`run_ws_connection` exists once per adapter and each does the same work: split the socket, pump a
channel to the write half, call `connect`, loop on the read half converting frames, dispatch to
`callbacks.message`, own and abort the spawned stream tasks, call `disconnect`. Measured with
`AUDIT3_DISPATCH.md` §4's method — normalise whitespace, substitute the adapter noun and its
library's `Message` type, split into units:

| pair | differing units |
| --- | --- |
| poem vs rocket | **1** — the stream type in the signature |
| salvo vs poem, salvo vs rocket | **5** |
| axum vs each of the others | 25–27 |
| tungstenite vs each of the others | 38–40 |

§4's collapse line is 0–6 units and its split line is 35–45. Three of the five sit inside the band
that memo drew for collapsing; the fifth sits on the line for keeping separate.

**Two of the divergences are not style.** axum's inbound-conversion arm is `Err(_) => {}` where
salvo, poem and rocket break — that one unit is F196, and it is why the adapter that appears to
discard a Close is the only one that answers it. And `catch_unwind` around the read loop exists on
axum and tungstenite only, so on the other three a panic unwinds past `callbacks.disconnect` and
leaves the client registered in `WsClientMap` and in the gateway's own map, with `on_disconnect`
never firing (F212). The trigger is narrow — every role a user writes is already wrapped inside
`GatewayWrapper` — and the point is that two adapters carry a guarantee three do not, with nothing
recording which posture is intended.

**The SPI under them is the odd one out in the framework.** `WsConnectionCallbacks` is three boxed
closures where `HttpAdapter`, `WsAdapter`, `RpcAdapter` and `GrpcAdapter` are traits, so the
signature cannot grow without changing a `dyn Fn` type — which is why F204's disconnect reason has
nowhere to go. And `MessageCallbackResult::{Continue, Stop, Stream}` is a second cardinality
vocabulary beside ADR-0049's `Items::{Empty, One, Many}`, mapped lossily: `Items::One` is sent by
core and reported as `Continue`, while `Items::Many` is handed to the adapter not to be sent — both
paths write to the same channel — but so the adapter can own the `JoinHandle` and abort it on
disconnect. That enum conflates *should I keep reading* with *here is a task for you to own*.

**The sink is copied too, which the first measurement missed.** `TokioSender` exists five times —
`crates/ulo-http-{axum,salvo,poem,rocket}/src/tokio_sender.rs` plus one inline in
`ulo-ws-tungstenite` — and four of them are `pub`. poem's and rocket's are byte-identical after
whitespace normalisation; poem's and salvo's differ by one word in a doc comment. Core already owns
the abstraction they all implement: `WsSink` (`crates/ulo/src/ws/broadcast.rs:25-31`). Only the read
side is adapter-owned, and every adapter reduces its socket to a `Stream`/`Sink` pair on the first or
second line by calling `.split()`.

**And core spawns nothing.** Zero `tokio::spawn` in `crates/ulo/src/ws/` and `application.rs`, against
16 in the adapters, each carrying `stream_tasks` / `JoinHandle` / `.abort()` bookkeeping.

This is where `HttpAdapter` was before the HTTP audit, and the answer is the same one: core owns the
loop, the adapter supplies the transport and declares its capabilities. `Dispatched::Upgrade` already
hands the adapter the callbacks, so half the seam is built by work already decided. D8 settles it.

The session store puts nothing on a wire beyond what §9 records.

## 12. The probe — what was measured, and against what

`artifacts/ws_wire_probe.rs` beside this memo. A hand-rolled client: base64 key, HTTP upgrade written
by hand, response head read a byte at a time so no frame bytes are swallowed, masked writes, and a
frame reader that returns `fin`, `rsv`, `opcode` and payload. No client library, because the question
is what went on the wire and a client library answers what it decided the wire meant.

Twelve cases: Ping/Pong, close echo, Ping-after-Close, subprotocol across four header shapes,
fragmentation, an interleaved control frame, four protocol errors, payload ceiling by bisection, the
refusal frames on four paths, life after a handler's own Close, the handshake contents, extension
negotiation, and Origin with and without `CorsMiddleware`.

**Two cases came back clean and are recorded as passes rather than omitted.** Fragmentation works on
all five: a text message split across a `fin=0` text frame and a `fin=1` continuation reassembles
correctly, and a Ping injected between the fragments is answered without disturbing the message,
which is §5.4's requirement. A handler that answers with a close ends the connection on all four
HTTP adapters, and a message sent afterwards is not answered — §5.5.1's *"MUST NOT send any more data
frames after sending a Close frame"* holds.

**The trap this probe was built to avoid.** `read_until(…, |f| f.opcode == 0x8)` returns `None` both
on timeout and on EOF, which reads the same in a result table. The first close-echo run reported
`NO CLOSE within 3s` for four adapters and that is not a finding — it conflates *the server said
nothing* with *the server hung up*. The case was rewritten to drain every frame, report which arrived,
and report separately whether the socket ended, and then again as a raw byte dump. F196 and F197 rest
on the byte dump.

## 13. Decisions

Each is a choice rather than a defect; the findings above are the evidence. **D1–D7 were put to the
user and settled on 2026-09-23** and are not open — a recommendation kept beside one is its argument,
not a choice still to make. **D8 was settled the same day**, after the spike below.

**D1 — SETTLED 2026-09-23: both.** F196 is a MUST and F197 a SHOULD, and they do not share a fix. Both are built.

*F196* is one extra poll: after a Close, read once more before leaving the loop, which is what axum
does by accident. Local to four adapters, no SPI change. There is no real alternative and no reason
to defer it.

*F197* needs ulo to compose a close for a protocol error, which means a mapping from the underlying
library's error to a §7.4.1 code — `1002` for a protocol fault, `1007` for invalid UTF-8, `1009` over
the ceiling. That mapping exists nowhere and each of the five adapters surfaces its library's error
as a different type, so it is five mappings or one normalised error on the SPI. **Recommended: build
F196 now and take F197's normalised read error with the callback widening F204 needs** — the
disconnect callback grows a parameter either way, and the reason a connection ended is the same value
the close code is derived from.

**D2 — SETTLED 2026-09-23: (a), and F211 folds in.** Whether ulo selects a subprotocol, and where. Three options. *(a)* A declared list on
`Gateway`, intersected with the parsed client list at the adapter — correct, and needs a pre-upgrade
decision point that does not exist. *(b)* Keep echoing and fix only the list-splitting, so
`chat, superchat` picks `chat` — closes F190's adapter half and leaves the framework agreeing to
protocols nothing implements. *(c)* Send no subprotocol ever, matching rocket and tungstenite —
uniform, conformant, and makes the GraphQL subscription gateway unserveable everywhere rather than on
two adapters. **SETTLED 2026-09-23: (a).** It is the only one that makes rocket and tungstenite work, and the
seam it needs is the same one D3 needs. Note that a subprotocol cannot be agreed to after the
handshake by any mechanism — unlike the Origin question, there is no frame that substitutes.

**F211 folds in here**, as the same rule one level down: the gateway declares, the framework applies.
`event_field() -> &str` becomes `route(&self, msg: &WsMessage) -> Option<String>` with today's JSON
lookup as the default body, and a binary protocol routes on whatever it carries. Two call sites, one
override to migrate.

**D3 — SETTLED 2026-09-23, and smaller than it was written.** The first version of this decision asked
whether a pre-upgrade hook should exist at all, and put its cost at an adapter SPI change across five
crates. That was written without reading `HTTP_AUDIT.md` §1, which already builds it: `dispatch(req)
-> Dispatched` decides `Respond` or `Upgrade` from the ulo `HttpRequest` before the handshake, spiked
on poem and salvo and falsified. F207 becomes `Respond(403)` and F205 becomes a value in the
request's `extensions`. Neither needs a seam of its own.

What is left is two refinements to a decided design:

1. **`Dispatched::Upgrade` carries no slot for a negotiated subprotocol.** It should carry one rather
   than the selection riding the 101 `HttpResponse` — the separate-port path has no `HttpResponse`
   and needs the same answer, so a slot on the variant is the shape both paths can share.
2. **Separate-port WebSocket bypasses HTTP dispatch entirely.** `WsAdapter::register_gateway` never
   reaches it and `accept_async` negotiates nothing. That half wants `accept_hdr_async`, whose
   callback sees the request and sets response headers — the same decision arriving by a different
   road.

This is not a reversal of ADR-0036. That record decides how a refusal is explained once a socket
exists, and holds either way.

**D4 — SETTLED 2026-09-23: move, on the standards argument.** Which close codes an `ErrorKind` maps to. The registry has `3000 Unauthorized`,
`3003 Forbidden` and `3008 Timeout`; ulo has `1008` for the first two and `1011` for the third. Moving
means a client reads a framework-range code, which is what the range is for and which no protocol
document defines. Staying means six kinds arrive as one code. The comment is wrong either way and is
corrected in both branches. **Recommended: move `Unauthorized` to `3000`, `Forbidden` to `3003` and
`Timeout` to `3008`, keep `1008` for the remaining policy kinds and `1013` where it is.** The
deviation test applies — the weight is one match arm, and what it buys is a client that can tell
authentication from validation. Both mappings are conformant; the registered codes are the more
precise reading and the range exists for a framework making this choice.

**D5 — SETTLED 2026-09-23: return a `Result`.** Refuse a forbidden close code at the constructor. `close_with` is the one place every close
ulo writes passes through. Refusing `1005`, `1006`, `1015` and anything under `1000` costs a branch.
The question is the shape of the refusal: clamp to `1008` and log, or return a `Result`. **Recommended:
return a `Result`** — the value is user-supplied, and clamping writes a different frame than the one
the handler asked for.

**D6 — SETTLED 2026-09-23: keep 16 MiB, make it configurable.** The WebSocket size ceiling. Decide a number, expose it, and answer `1009` on breach. The
number is presently 16 MiB by inheritance. **Recommended: keep 16 MiB as the default** — it is what
every deployment on this framework has run against — and make it settable. The `1009` needs D1's
second half, because an oversize frame is a read error and nothing composes a close for one.

**D7 — SETTLED 2026-09-23: unify the surface, and give the wire its own type.** As first written this
decision asked only whether the Redis envelope should keep deriving off `WsMessage`. That is half of
it. §9 is the other half, and the two are one subject: **coherence of the broadcast surface.**

*The surface.* One `#[async_trait] trait Broadcast`, implemented by both services and exported in
place of the concrete types. Async throughout, because the distributed implementation cannot be
synchronous and async over a local `RwLock` costs nothing. `send(&self, …)` on both — Redis takes
`mut self` only to reach `&mut publisher` and can clone internally. Then `RedisBroadcastModule` is
the module swap its documentation already claims.

*The count.* `send` answers `Result<(), BroadcastError>`. Redis's `Ok(0)` is a literal falsehood and
core's `Ok(n)` counts channel sends rather than frames delivered, so the number is a promise neither
implementation keeps. A caller wanting local fan-out asks `get_room_clients().len()`, which is a
question with an answer.

*The wire.* `ulo-ws-redis` gets its own `BroadcastFrame` with an explicit `#[serde(tag = …)]`, bytes
rather than a JSON number array for binary, and a version field. That removes the fourfold expansion
and decouples the cross-process format from `WsMessage` growing variants under F208's
`#[non_exhaustive]` — which is the coupling that makes two processes across a rolling deploy drop
each other's frames. A wire format and an in-process enum have different reasons to change, and one
derive currently ties them together.

*The membership operations* come in scope with this. They carry no RFC 6455 question, and they are
where the sync/async split shows.

**D8 — SETTLED 2026-09-23: core owns the WebSocket connection loop, and it is designed with `Dispatched` rather than after it.**

§11's measurement puts three of the five loops inside `AUDIT3_DISPATCH.md` §4's collapse band, and
F196, F197, F204 and F212 are what a five-times-copied loop produced. Four probes decided it:

- **The abstraction already exists five times, unnamed.** Every adapter calls `.split()` on the first
  or second line of `run_ws_connection`, so each already reduces its library's socket to a
  `Stream`/`Sink` pair. What remains adapter-specific after that is the two conversion functions,
  which are already named per-adapter functions.
- **Core already owns the write side.** `trait WsSink` (`crates/ulo/src/ws/broadcast.rs:25-31`) is the
  sink abstraction; only the read side is adapter-owned, and the asymmetry rests on nothing.
- **`TokioSender` is five copies**, four of them `pub` (`crates/ulo-http-{axum,salvo,poem,rocket}/src/tokio_sender.rs`)
  and one inline in `ulo-ws-tungstenite`. poem's and rocket's are byte-identical after whitespace
  normalisation; poem's and salvo's differ by one word in a doc comment. F215 measured the loop and
  missed the sink.
- **Core spawns nothing today** — zero `tokio::spawn` in `crates/ulo/src/ws/` and `application.rs` —
  while the adapters spawn 16 times and carry `stream_tasks` / `JoinHandle` / `.abort()` bookkeeping
  in all five files.

**The shape.** The adapter supplies a transport — its split halves plus its two conversions — and core
runs the connection. `Dispatched::Upgrade` carries a `WsUpgrade` rather than
`Arc<WsConnectionCallbacks>`: it names the subprotocol core selected, which is what the adapter needs
to write the 101, and it accepts the transport, which is what core needs to serve. **One type answers
D3's remaining half and D8 at once**, instead of bolting a subprotocol slot onto a callbacks bundle.

`MessageCallbackResult` collapses into `Items` plus whatever names task ownership, which is the only
thing its `Stream` variant is for; core merges reply streams into its own loop with
`FuturesUnordered` instead of the adapters spawning, which keeps
`feedback-keep-runtime-out-of-core` and deletes the abort bookkeeping from five files.

**What this buys beyond deduplication.** F196's fix stops being a coincidence. Under the
poll-once-more remedy it works because of an undocumented `tungstenite` internal — the flush at the
top of the next `read()`. Under a transport trait, core calls `close(frame)` and each adapter does
what its library requires, which is a fix rather than a behaviour being relied on.

**Why with the HTTP build and not after it.** An earlier revision of this decision said D8 should
follow that build so it copies a settled shape. It cannot: D8 changes the payload of a variant that
build introduces. Shipping `Upgrade(Arc<WsConnectionCallbacks>)` and then replacing
`WsConnectionCallbacks` breaks one enum twice in a single release. The HTTP build decides
`Dispatched`; D8 decides what `Upgrade` carries; that is one conversation.

**The cost, stated.** Five adapter crates rewritten at their connection seam — in a pass already
rewriting them at their close handling, which is why the marginal cost is smaller than the total. The
error normalisation across five libraries is needed either way; D8 makes it the SPI rather than a
bolt-on. Phase 1's `catch_unwind` stop-gap and Phase 2's callback widening stop being work at all.

**The spike — run 2026-09-23, passed on all five.** The design rests on core driving each adapter's
read half generically, which needs `Send + 'static`. Every adapter already `tokio::spawn`s its write
half, which forces those bounds on that side; the read halves are awaited inline today, so the bound
was probable and unproven. Each crate was patched with a `Send + 'static` assertion over
`SplitStream<T>` and `SplitSink<T, M>` for its own socket type and a generic function carrying the
bounds a core loop needs:

| adapter | socket type | result |
| --- | --- | --- |
| axum | `axum::extract::ws::WebSocket` | pass |
| salvo | `salvo::websocket::WebSocket` | pass |
| poem | `poem::web::websocket::WebSocketStream` | pass |
| rocket | `rocket_ws::stream::DuplexStream` | pass |
| tungstenite | `tokio_tungstenite::WebSocketStream<TcpStream>` | pass |

**Falsified before being believed.** The same assertion applied to `std::rc::Rc<()>` fails to compile
with *"`Rc<()>` cannot be sent between threads safely"* (E0277), so a passing run is a result rather
than a vacuous bound. A first attempt proved nothing and is recorded because it looked like a result:
the patch script split its type specifications on `:`, which collided with every `::` in a path, and
five crates failed with `expected type, found crate` — an error about the harness reading as an
error about the subject. The branch is reverted; the diff is kept in this session's scratchpad as
`spike-d8-transport-bounds.patch`.

## What is out of scope, and why

- **Guards, interceptors and the error chain on WebSocket.** The pipeline audit took them across all
  four transports and settled them: its build's **PR 8** makes a guard's refusal one event on both
  WebSocket phases (F130) and **PR 10** is F131. Excluding them here avoids two passes owning one
  surface, not work going undone. What that pass left is the wire and the SPI, and §11 takes the SPI.
- **`permessage-deflate`.** Not a conformance question — §4.2.2 makes declining every extension
  conformant — and not a knob ulo is withholding. F209 carries the mechanism: `tungstenite` rejects
  any frame with a reserved bit set and has no negotiated-extension knob, so compression has to live
  in the frame codec, and on four of five adapters ulo does not own it. The upstream record is in
  that entry. The decision is to declare the position through `negotiates_extensions: false` rather
  than leave silence a reader cannot distinguish from an oversight; the trigger is `tungstenite`
  accepting its open PR.
- **The GraphQL subscription gateway's protocol rules.** F187, F188 and F189 belong to the GraphQL
  pass, which filed them. F190's adapter half is F198 and F199 here.

## Couplings the triage pass should carry

- **D3 consumes the HTTP build, and does not duplicate it.** `Dispatched` is that build's PR 1–9.
  The WebSocket work adds a subprotocol slot to `Dispatched::Upgrade` and reaches the separate-port
  path through `accept_hdr_async`. Sequencing it before the HTTP build means designing the same seam
  twice.
- **D8 should follow the HTTP build for the same reason.** It copies a shape that build settles.
- **F198/F199 are F190's adapter half.** The GraphQL audit filed the gateway half. One fix, sequenced
  together; the gateway half is one comparison.
- **F201 blocks the GraphQL build's PR 6.** That PR routes graphql-transport-ws's five closures
  through `WsError::Refused`, which on the message path carries no code to the peer.
- **F206 is the fifth site of the four-query-parser coupling.** ADR-0052 picks the crate; whoever
  runs first settles it for five parsers, not four.
- **D1's second half and F204 widen the same callback**, and under D8 both are absorbed by the trait
  that replaces it. Building D8 first would make two of this pass's PRs unnecessary; building it
  later makes them throwaway work. That is the sequencing question the triage pass owns.
- **F36 is answerable by promoting the probe.** Eleven server-side cases stamped per adapter.

## What is owed

`WS_BUILD_HANDOFF.md`, beside this file. It carries the PRs; this file carries the reasoning and the
decisions. D1–D8 are settled; D8 is what the build handoff's Phase 3 builds.
