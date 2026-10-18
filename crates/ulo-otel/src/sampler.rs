use opentelemetry_sdk::trace::Sampler;

use crate::options::OtelOptions;

/// Parent-based sampling: the parent's decision stands, and a remote parent with the sampled flag
/// clear exports nothing. A root span is sampled by the options' ratio.
pub(crate) fn sampler(options: &OtelOptions) -> Sampler {
    let _ = options;
    todo!()
}
