use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use opentelemetry::Context;
use opentelemetry::trace::{SpanContext, SpanId, TraceContextExt, TraceFlags, TraceId, TraceState as OtelState};
use opentelemetry_sdk::trace::{IdGenerator, RandomIdGenerator};
use tracing::Span;
use tracing_opentelemetry::{OpenTelemetrySpanExt, SetParentError};
use ulo::{TraceContext, TraceOrigin, TraceSource, TraceState};

/// Set once the warning that spans reach no OpenTelemetry layer has been logged; `start` runs on
/// every call served, and one warning says what every later call would.
static UNLAYERED: AtomicBool = AtomicBool::new(false);

/// The `TraceSource` implementation. `start` converts the inbound context into an
/// `opentelemetry::Context`, sets it with `OpenTelemetrySpanExt::set_parent` before the span is first
/// entered, and reads `span.context()` back as the execution's own context, which `set_trace` marks
/// `Originated`.
///
/// An absent inbound context, or an invalid one, which the transport hands in as absent, leaves the
/// span's parent to `tracing`'s own parenting: the span it was created inside, or an explicit
/// parent its creator named, and with neither, the root of a new trace.
///
/// When the span reaches no OpenTelemetry span, because no `Otel::layer` is installed or a filter
/// disabled it, nothing was recorded here: the answer is the inbound context unchanged, or with
/// none a new trace whose sampled flag is clear.
pub(crate) struct OtelSource {
    /// Set by the flush hook before it shuts the provider down, after which every span the tracer
    /// starts is empty by design and the warning below would be wrong.
    pub(crate) flushing: Arc<AtomicBool>,
}

impl TraceSource for OtelSource {
    fn start(&self, span: &Span, inbound: Option<&TraceContext>) -> TraceContext {
        if let Some(inbound) = inbound {
            let parent = Context::new().with_remote_span_context(parent_of(inbound));
            if let Err(SetParentError::AlreadyStarted) = span.set_parent(parent) {
                tracing::debug!(
                    "a transport span was entered before ulo-otel's TraceSource ran; it keeps the parent it started with"
                );
            }
        }
        // `context` starts the OpenTelemetry span, which fixes its parent, so it follows
        // `set_parent`.
        let own = span.context();
        let started = own.span();
        let started = started.span_context();
        if started.is_valid()
            && let Some(answer) = TraceContext::new(
                started.trace_id().to_bytes(),
                started.span_id().to_bytes(),
                started.trace_flags().to_u8(),
                state_from(started.trace_state()),
            )
        {
            return answer;
        }
        // A disabled span is one the application filtered out, which needs no warning.
        if !span.is_disabled()
            && !self.flushing.load(Ordering::Acquire)
            && !UNLAYERED.swap(true, Ordering::Relaxed)
        {
            tracing::warn!(
                "ulo-otel's TraceSource is bound, but the transports' spans reach no OpenTelemetry layer: \
                 install `Otel::layer()` in the subscriber, outside any `reload` layer"
            );
        }
        unrecorded(inbound)
    }
}

/// The inbound context as the parent the tracer reads. One a transport received is remote; an
/// `Originated` one is a span of this process, the execution's first, which a second span opened
/// for the execution continues.
fn parent_of(inbound: &TraceContext) -> SpanContext {
    SpanContext::new(
        TraceId::from_bytes(inbound.trace_id()),
        SpanId::from_bytes(inbound.span_id()),
        TraceFlags::new(inbound.flags()),
        inbound.origin() == TraceOrigin::Received,
        state_into(inbound.state()),
    )
}

/// The core's list has passed W3C Trace Context's grammar; a member OpenTelemetry's own check
/// refuses all the same drops the list, as a recipient discards an invalid one.
fn state_into(state: &TraceState) -> OtelState {
    OtelState::from_key_value(state.entries()).unwrap_or_default()
}

fn state_from(state: &OtelState) -> TraceState {
    let header = state.header();
    TraceState::parse([header.as_str()])
}

fn unrecorded(inbound: Option<&TraceContext>) -> TraceContext {
    if let Some(inbound) = inbound {
        return inbound.clone();
    }
    let ids = RandomIdGenerator::default();
    // `TraceContext::new` refuses an all-zero ID, which the generator yields with probability
    // 2^-64 per draw.
    loop {
        if let Some(fresh) = TraceContext::new(ids.new_trace_id().to_bytes(), ids.new_span_id().to_bytes(), 0, TraceState::new()) {
            return fresh;
        }
    }
}
