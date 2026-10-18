use opentelemetry_sdk::trace::Sampler;

use crate::options::OtelOptions;

/// Parent-based sampling: the parent's decision stands, and a remote parent with the sampled flag
/// clear exports nothing. A root span is sampled by the options' ratio, every one when it is unset.
pub(crate) fn sampler(options: &OtelOptions) -> Sampler {
    let root = match options.sample_ratio {
        None => Sampler::AlwaysOn,
        Some(ratio) => Sampler::TraceIdRatioBased(ratio),
    };
    Sampler::ParentBased(Box::new(root))
}
