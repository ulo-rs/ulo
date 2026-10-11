//! The runtime half of a link on tokio.

use std::future::Future;

use ulo_tokio::Tokio;

use crate::Harness;

/// Tokio as a broker's [`Harness`]: each scenario on a multi-thread runtime of its own.
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
}
