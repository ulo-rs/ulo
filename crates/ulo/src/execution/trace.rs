//! W3C Trace Context on the execution (integrations §6.1, I5, I7): the context a call carries,
//! parsed and written by the rules of W3C Trace Context Level 1, the Recommendation, with no
//! OpenTelemetry dependency, no clock and no randomness. Generating IDs belongs to a tracer, which
//! reaches an execution through [`TraceSource`].
//!
//! Section numbers below are the Recommendation's: the `traceparent` grammar and its
//! forward-compatible versions in §3.2, `tracestate`'s in §3.3, the pass-through rule in §3.4 and
//! the mutation rules in §3.5.

use std::fmt;

use tracing::Span;

/// The sampled flag, the one flag Level 1 defines and the one a context originated here writes.
const SAMPLED: u8 = 0x01;
/// §3.3.1: a list holds at most 32 members.
const MAX_MEMBERS: usize = 32;
/// §3.3.1.5: at least 512 characters of the combined header are propagated.
const PROPAGATED: usize = 512;
/// §3.3.1.5: entries longer than this are removed first when the list is truncated.
const LONG_ENTRY: usize = 128;
/// Version `00`'s length, and the shortest any version may be.
const LEN_00: usize = 55;

/// Whether the app reads and writes W3C trace headers, set with `AppBuilder::trace`.
///
/// `On`, the default, propagates without a tracer: with no [`TraceSource`] bound, an incoming
/// context is forwarded exactly as received, so a ulo service never breaks a trace that crosses
/// it. `Off` reads and writes nothing: `Execution::open` drops a context a transport passed in,
/// no source is called, and `traceparent` and `tracestate` are ordinary headers to every client.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TraceMode {
    #[default]
    On,
    Off,
}

/// Where an execution's context came from, which decides how a client writes it out (§6.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TraceOrigin {
    /// Seeded through `ExecOptions::trace`: extracted by a transport, or handed to a standalone
    /// execution. Forwarded as received, both headers' text, and a caller's own `tracestate` on a
    /// call forwarding it is refused, since only a participant may modify the list (§3.4).
    Received,
    /// Written through `Execution::set_trace`, the answer of a [`TraceSource`]: originated or
    /// modified here. Written out as version `00` with the sampled flag as its one flag, and a
    /// caller's `tracestate` entries are merged into it by §3.5.
    Originated,
}

/// A trace context: a trace ID, a span ID, the flags as bits and the trace state.
///
/// A context [`extract`](Self::extract)ed from headers keeps both headers' text as received
/// beside the parsed fields. A `Received` context with that text is written out as the text,
/// [`traceparent`](Self::traceparent) and [`tracestate`](Self::tracestate) answering it unchanged,
/// a higher version and Level 2's `random` flag included; any other is written as version `00`,
/// its flags reduced to the sampled flag.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TraceContext {
    trace_id: [u8; 16],
    span_id: [u8; 8],
    flags: u8,
    state: TraceState,
    origin: TraceOrigin,
    received: Option<ReceivedText>,
}

/// The two headers as they arrived, each value trimmed of the whitespace around it, which is not
/// part of a field value.
#[derive(Clone, Debug, PartialEq, Eq)]
struct ReceivedText {
    traceparent: String,
    /// Every `tracestate` field joined with `,`, as a recipient may combine them; `None` when no
    /// field carried anything.
    tracestate: Option<String>,
}

impl TraceContext {
    /// The context `traceparent` and `tracestate` carry, given every field value received under
    /// each name, or `None` when there is none to continue: a missing or duplicated `traceparent`,
    /// one off the grammar, version `ff`, uppercase hex, or an all-zero trace or span ID. The
    /// specification calls that a restart; a recipient with a tracer starts a new trace, and one
    /// without carries no context. A `tracestate` is parsed only beside a valid `traceparent`,
    /// and an invalid one leaves the state empty without affecting the `traceparent`.
    ///
    /// A version above `00` is read by version `00`'s offsets, provided the value is at least as
    /// long and the flags are followed by its end or a dash (§3.2.4). The context answers
    /// `Received`.
    pub fn extract<'a, 'b>(
        traceparent: impl IntoIterator<Item = &'a str>,
        tracestate: impl IntoIterator<Item = &'b str>,
    ) -> Option<TraceContext> {
        let mut fields = traceparent.into_iter();
        let (Some(only), None) = (fields.next(), fields.next()) else { return None };
        let only = trim_ows(only);
        let (trace_id, span_id, flags) = parse_traceparent(only)?;
        let fields: Vec<&str> = tracestate.into_iter().map(trim_ows).filter(|field| !field.is_empty()).collect();
        let state = TraceState::parse(fields.iter().copied());
        let tracestate = (!fields.is_empty()).then(|| fields.join(","));
        Some(TraceContext {
            trace_id,
            span_id,
            flags,
            state,
            origin: TraceOrigin::Received,
            received: Some(ReceivedText { traceparent: only.to_owned(), tracestate }),
        })
    }

    /// A context from its parts, for a [`TraceSource`] reading its own span's IDs back. `None`
    /// for an all-zero trace or span ID, which the specification makes invalid. It carries no
    /// header text and answers `Originated`; `Execution::set_trace` marks it so in any case.
    pub fn new(trace_id: [u8; 16], span_id: [u8; 8], flags: u8, state: TraceState) -> Option<TraceContext> {
        if trace_id == [0; 16] || span_id == [0; 8] {
            return None;
        }
        Some(TraceContext { trace_id, span_id, flags, state, origin: TraceOrigin::Originated, received: None })
    }

    pub fn trace_id(&self) -> [u8; 16] {
        self.trace_id
    }

    /// The span the next hop's span is a child of: the caller's for a context forwarded as
    /// received, the execution's own once a `TraceSource` answered.
    pub fn span_id(&self) -> [u8; 8] {
        self.span_id
    }

    /// The flags as received or as the source answered them, every bit kept; what is written out
    /// follows [`traceparent`](Self::traceparent).
    pub fn flags(&self) -> u8 {
        self.flags
    }

    pub fn sampled(&self) -> bool {
        self.flags & SAMPLED != 0
    }

    pub fn state(&self) -> &TraceState {
        &self.state
    }

    pub fn origin(&self) -> TraceOrigin {
        self.origin
    }

    /// The `traceparent` value to forward: the text as received for a `Received` context that has
    /// one, otherwise version `00` in lowercase hex with the sampled flag as its one flag, every
    /// other bit written as zero (§3.2.2.5, §4.3).
    pub fn traceparent(&self) -> String {
        if let (TraceOrigin::Received, Some(received)) = (self.origin, &self.received) {
            return received.traceparent.clone();
        }
        let mut out = String::with_capacity(LEN_00);
        out.push_str("00-");
        push_hex(&mut out, &self.trace_id);
        out.push('-');
        push_hex(&mut out, &self.span_id);
        out.push('-');
        push_hex(&mut out, &[self.flags & SAMPLED]);
        out
    }

    /// The `tracestate` value to forward, `None` when there is nothing to write: the text as
    /// received for a `Received` context that has one, unchanged as §3.4 requires of a list passed
    /// through with its `traceparent`, otherwise the parsed list.
    pub fn tracestate(&self) -> Option<String> {
        if let (TraceOrigin::Received, Some(received)) = (self.origin, &self.received) {
            return received.tracestate.clone();
        }
        (!self.state.is_empty()).then(|| self.state.to_string())
    }

    /// The same context marked `Received`, as `ExecOptions::trace` seeds it.
    pub(crate) fn received(self) -> TraceContext {
        TraceContext { origin: TraceOrigin::Received, ..self }
    }

    /// The same context marked `Originated`, as `Execution::set_trace` writes it. The text as
    /// received describes the hop before this one and no longer applies, so it is dropped.
    pub(crate) fn originated(self) -> TraceContext {
        TraceContext { origin: TraceOrigin::Originated, received: None, ..self }
    }
}

/// A `tracestate` list: `key=value` members, at most 32, each key once, in order (§3.3).
/// `Display` writes the header value, members joined by `,`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TraceState {
    entries: Vec<(String, String)>,
}

impl TraceState {
    pub fn new() -> TraceState {
        TraceState::default()
    }

    /// The list every field value under `tracestate` carries, the fields read as one list in
    /// order (RFC 9110 §5.3). Whitespace around a member is ignored and an empty member skipped.
    /// More than 32 members, or any member off the key or value grammar, discards the whole list,
    /// as the W3C test suite's strict level expects; a key repeated keeps its leftmost entry.
    pub fn parse<'a>(fields: impl IntoIterator<Item = &'a str>) -> TraceState {
        let mut entries: Vec<(String, String)> = Vec::new();
        let mut members = 0;
        for field in fields {
            for member in field.split(',') {
                let member = trim_ows(member);
                if member.is_empty() {
                    continue;
                }
                members += 1;
                if members > MAX_MEMBERS {
                    return TraceState::default();
                }
                let Some((key, value)) = member.split_once('=') else { return TraceState::default() };
                if !key_valid(key) || !value_valid(value) {
                    return TraceState::default();
                }
                if !entries.iter().any(|(k, _)| k == key) {
                    entries.push((key.to_owned(), value.to_owned()));
                }
            }
        }
        TraceState { entries }
    }

    /// Every member, leftmost first.
    pub fn entries(&self) -> impl Iterator<Item = (&str, &str)> {
        self.entries.iter().map(|(key, value)| (key.as_str(), value.as_str()))
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.entries.iter().find(|(k, _)| k == key).map(|(_, value)| value.as_str())
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The list with `key=value` added as §3.5 has a vendor add its own: at the left, an older
    /// entry for the key dropped, then the list truncated to 32 members and 512 characters by
    /// whole entries, those over 128 characters first and then from the right, never the entry
    /// added. `None` when the key or the value is off the grammar.
    pub fn with_entry(&self, key: &str, value: &str) -> Option<TraceState> {
        if !key_valid(key) || !value_valid(value) {
            return None;
        }
        let mut entries = vec![(key.to_owned(), value.to_owned())];
        entries.extend(self.entries.iter().filter(|(k, _)| k != key).cloned());
        Some(TraceState { entries: truncated(entries, 1) })
    }

    /// The list with `front`'s members at the left, in their order, as a caller's entries enter
    /// an originated context's list (§3.5, integrations §6.3): a key in both moves left with
    /// `front`'s value, the entries of `self` that `front` does not name follow in their order,
    /// and the list is truncated as [`with_entry`](Self::with_entry) truncates it, `front`'s
    /// members the last removed.
    pub fn prepend(&self, front: &TraceState) -> TraceState {
        let mut entries = front.entries.clone();
        entries.extend(self.entries.iter().filter(|(k, _)| front.get(k).is_none()).cloned());
        TraceState { entries: truncated(entries, front.entries.len()) }
    }
}

impl fmt::Display for TraceState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, (key, value)) in self.entries.iter().enumerate() {
            if i > 0 {
                f.write_str(",")?;
            }
            write!(f, "{key}={value}")?;
        }
        Ok(())
    }
}

/// The parent of an execution's spans, as a tracer sees it: the hook a transport's span calls
/// between creating the span and first entering it (integrations §6.4, I7).
///
/// Bind one as `dyn TraceSource`, a singleton visible from the root module: the app resolves it
/// once, at `connect`, after every singleton is built, and `AppHandle::trace_source()` answers it
/// from then on. One a lazily loaded module binds later is not read. With none bound, nothing is
/// called and an incoming context is forwarded as received; under `TraceMode::Off` none is read.
///
/// `ulo_transport::span::call` calls [`start`](Self::start) with the span it just created and the
/// execution's inbound context, and a standalone `execute` handed a span through
/// `ExecOptions::span` calls it likewise; each records the answer through
/// `Execution::set_trace`, which marks it `Originated`.
pub trait TraceSource: Send + Sync + 'static {
    /// Makes `span` a child of `inbound`, or the root of a new trace when `inbound` is `None`, and
    /// answers the context the execution's outgoing calls carry: `span`'s own trace and span IDs.
    /// Called before `span` is first entered, the last point at which a tracer can still set its
    /// parent. Synchronous, and called on every call a transport serves.
    fn start(&self, span: &Span, inbound: Option<&TraceContext>) -> TraceContext;
}

fn trim_ows(value: &str) -> &str {
    value.trim_matches([' ', '\t'])
}

/// The trace ID, span ID and flags of one `traceparent` value already trimmed, or `None` for a
/// restart (§3.2.2, §3.2.4).
fn parse_traceparent(value: &str) -> Option<([u8; 16], [u8; 8], u8)> {
    let b = value.as_bytes();
    if b.len() < 3 || b[2] != b'-' {
        return None;
    }
    let [version] = hex::<1>(&b[0..2])?;
    if version == 0xff || b.len() < LEN_00 || b[35] != b'-' || b[52] != b'-' {
        return None;
    }
    let trace_id = hex::<16>(&b[3..35])?;
    let span_id = hex::<8>(&b[36..52])?;
    let [flags] = hex::<1>(&b[53..55])?;
    if version == 0 {
        if b.len() != LEN_00 {
            return None;
        }
    } else if b.len() > LEN_00 && b[LEN_00] != b'-' {
        return None;
    }
    if trace_id == [0; 16] || span_id == [0; 8] {
        return None;
    }
    Some((trace_id, span_id, flags))
}

/// `N` bytes from exactly `2 * N` lowercase hex digits; uppercase is invalid (§3.2.2).
fn hex<const N: usize>(digits: &[u8]) -> Option<[u8; N]> {
    if digits.len() != N * 2 {
        return None;
    }
    let mut out = [0u8; N];
    for (i, pair) in digits.chunks_exact(2).enumerate() {
        out[i] = (nibble(pair[0])? << 4) | nibble(pair[1])?;
    }
    Some(out)
}

fn nibble(digit: u8) -> Option<u8> {
    match digit {
        b'0'..=b'9' => Some(digit - b'0'),
        b'a'..=b'f' => Some(digit - b'a' + 10),
        _ => None,
    }
}

fn push_hex(out: &mut String, bytes: &[u8]) {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    for byte in bytes {
        out.push(char::from(DIGITS[usize::from(byte >> 4)]));
        out.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
}

/// §3.3.1.3: `lcalpha 0*255( lcalpha / DIGIT / "_" / "-"/ "*" / "/" )`, or
/// `tenant-id "@" system-id`, a tenant of 1 to 241 characters starting with a lowercase letter or
/// a digit and a system of 1 to 14 starting with a lowercase letter.
fn key_valid(key: &str) -> bool {
    let rest = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '-' | '*' | '/');
    match key.split_once('@') {
        None => {
            let mut chars = key.chars();
            key.len() <= 256 && chars.next().is_some_and(|c| c.is_ascii_lowercase()) && chars.all(rest)
        }
        Some((tenant, system)) => {
            let mut t = tenant.chars();
            let mut s = system.chars();
            tenant.len() <= 241
                && system.len() <= 14
                && t.next().is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
                && t.all(rest)
                && s.next().is_some_and(|c| c.is_ascii_lowercase())
                && s.all(rest)
        }
    }
}

/// §3.3.1.3: 1 to 256 printable ASCII characters other than `,` and `=`, the last not a space.
fn value_valid(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value.bytes().all(|b| (0x20..=0x7e).contains(&b) && b != b',' && b != b'=')
        && !value.ends_with(' ')
}

fn entry_len((key, value): &(String, String)) -> usize {
    key.len() + 1 + value.len()
}

fn header_len(entries: &[(String, String)]) -> usize {
    entries.iter().map(entry_len).sum::<usize>() + entries.len().saturating_sub(1)
}

/// `entries` cut to 32 members, then to 512 characters by whole entries (§3.3.1.5): an entry over
/// 128 characters goes first, then entries from the right. The first `keep` entries, the ones a
/// writer just added, go last, and the leftmost entry never.
fn truncated(mut entries: Vec<(String, String)>, keep: usize) -> Vec<(String, String)> {
    entries.truncate(MAX_MEMBERS);
    let keep = keep.clamp(1, entries.len().max(1));
    while header_len(&entries) > PROPAGATED && entries.len() > 1 {
        let long = |from: usize| (from..entries.len()).find(|&i| entry_len(&entries[i]) > LONG_ENTRY);
        let at = long(keep).or_else(|| (entries.len() > keep).then(|| entries.len() - 1)).or_else(|| long(1));
        match at {
            Some(i) => {
                entries.remove(i);
            }
            None => {
                entries.pop();
            }
        }
    }
    entries
}
