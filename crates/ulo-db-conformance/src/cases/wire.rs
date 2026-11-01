use ulo::{Redacted, StartupError, WiringError, is_permanent};

use crate::cases::{Root, builder, store};
use crate::{Host, PASSWORD, report};

/// A malformed URL is refused at `wire`, the password redacted. Planted break: a password echoed in
/// the refusal.
///
/// The host's reachable options wire first, so the refusal that follows is the malformed URL's and
/// not some other wiring error. The refusal is `StartupError::Wiring` holding the
/// `WiringError::ValueFailed` that `try_value` records for `Store::validate`'s problems, it is
/// permanent, and no rendering of it, the whole report, its `Debug` or any recorded value failure,
/// prints [`PASSWORD`]. `wire` builds nothing, so the registry holds no secret yet and nothing
/// scrubs a password `validate` writes: the store has to write its problems from the failure's
/// category.
pub async fn malformed_url_is_refused<H: Host>() {
    let host = H::start().await;

    if let Err(error) = builder::<H>(Root::new().import(store::<H>(host.options()))).wire() {
        crate::startup_failed!("the host's reachable options did not wire: {}", report(&error));
    }

    let error = match builder::<H>(Root::new().import(store::<H>(host.malformed_options(PASSWORD)))).wire() {
        Ok(_) => panic!("malformed options wired: `{}`'s `validate` accepted them", <H::Store as ulo_db::Store>::NAME),
        Err(error) => error,
    };
    let StartupError::Wiring(errors) = &error else {
        panic!("malformed options were refused outside `wire`'s report: {}", report(&error));
    };
    let failures: Vec<&Redacted> = errors
        .iter()
        .filter_map(|entry| match entry {
            WiringError::ValueFailed { error, .. } => Some(error),
            _ => None,
        })
        .collect();
    assert!(
        !failures.is_empty(),
        "malformed options were refused at `wire` without a recorded value failure, the shape `try_value` gives `validate`'s problems: {error}"
    );
    assert!(is_permanent(&error), "a wiring refusal is a misconfiguration, and `is_permanent` read it as retryable: {error}");

    crate::cases::assert_redacted(&error, "the refusal of a malformed URL");
    for failure in failures {
        for text in [failure.to_string(), format!("{failure:?}")] {
            assert!(!text.contains(PASSWORD), "the recorded value failure prints the password: {text}");
        }
    }
}
