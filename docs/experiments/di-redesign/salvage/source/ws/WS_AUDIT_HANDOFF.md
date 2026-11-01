# Handoff — the WebSocket wire audit

Run this as its own session. It gates nothing and can run at any point, including while the HTTP
build is in flight.

## Scope

The WebSocket wire against RFC 6455, and the parts of the WebSocket surface the standards audit
named as unexamined: subprotocol negotiation (`Sec-WebSocket-Protocol`), the close-code registry
against ulo's 1008/1011/1013 choices, fragmentation, and payload limits. Five implementors —
`ulo-http-axum`, `ulo-http-poem`, `ulo-http-salvo`, `ulo-http-rocket` and `ulo-ws-tungstenite` —
and the wire is what each of them produces.

Also in scope because nothing else will take it: `ulo-ws-redis`, the broadcast surface, and the
session store, to the extent they put bytes on a wire.

Out of scope: guards, interceptors and the error chain on WebSocket — that is the pipeline audit,
which takes all four transports at once.

## Start here — the only open MUST in the framework

**F67.** RFC 6455 §5.5.2: *"Upon receipt of a Ping frame, an endpoint MUST send a Pong frame in
response, unless it already received a Close frame."* ulo sends no Pong on any path, so conformance
rests on the library underneath — tokio-tungstenite under four adapters, rocket_ws under the fifth.
**Believed true for all five and established for none.** #310 made the gateway *drop* Ping frames,
which is correct if and only if the library answers, so that fix carries the same assumption.

The test is per adapter: connect, send a Ping, assert a Pong arrives within a bound. That is the
first thing this session should run, before reading anything else.

## Already filed — read before probing

- **F67** — the Ping MUST, above.
- **F66** — control-frame handling split three ways, and no keepalive. Carries three findings on
  one subject, including that `WsMessage::Close(_)`'s guard arm added in #310 is unreachable
  through every shipped adapter.
- **F36** — `WebSocketAdapter` has four implementors and no conformance suite, where `HttpAdapter`
  has four suites and `RpcAdapter` was brought into line by #258. The rule it violates is written
  down in `docs/explainers/testing-and-examples.md:84`.
- `STANDARDS_AUDIT.md` §2 carries W1–W5; W1 became F66 and W5 became F67. **W2, W3 and W4 are not
  filed** — a gateway cannot construct a Ping or Pong, cannot observe a Pong, and `WsMessage` is
  closed against a protocol that reserves opcodes `0x3`–`0x7` and `0xB`–`0xF`. File them.

## Method

- **Probe, then falsify.** Disable the probe and re-run to confirm it measured what it claimed. A
  clean first run proves nothing. Raw frames beat a client library where the question is what went
  on the wire.
- **Fetch the spec, do not recall it.** `curl -sS https://www.rfc-editor.org/rfc/rfc6455.txt` into
  the scratchpad, then `grep -n -A`. `WebFetch` truncates long RFCs and reports the sections as
  absent. On the HTTP pass two claims written from memory were wrong, one of them a section number.
- **Confidence marking per claim** — Probed / Read / Spec / Unverified — declared near the top.
- The three-category test in `project-the-spec-conformance-seam` places each finding: RFC 6455
  decides it, leaves it open, or never mentions it. The close-code choices are mostly the second
  kind, which means ulo must choose once rather than per adapter.

## Deliverable

1. `WS_AUDIT.md` at this level. Form: `feedback-the-audit-memo-form` in memory.
2. Findings to `FRAMEWORK_GAPS.md`, numbered from the highest there.
3. Its own build handoff, in the shape of `HTTP_BUILD_HANDOFF.md`.

Nothing is committed by an audit.
