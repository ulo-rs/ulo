//! The runtime half of a link on tokio.

use std::future::Future;

use ulo_runtime_tokio::Tokio;

use crate::Harness;

/// Tokio as a broker's [`Harness`]: each scenario on a multi-thread runtime of its own, and the one
/// that runs on one thread on a current-thread runtime of its own.
pub struct OnTokio;

impl Harness for OnTokio {
    type Runtime = Tokio;

    fn runtime() -> Tokio {
        Tokio::current()
    }

    fn block_on<F: Future>(fut: F) -> F::Output {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap_or_else(|error| crate::startup_failed!("a tokio runtime for the scenario: {error}"))
            .block_on(fut)
    }

    fn block_on_one_thread<F: Future>(fut: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap_or_else(|error| crate::startup_failed!("a current-thread tokio runtime for the scenario: {error}"))
            .block_on(fut)
    }
}
