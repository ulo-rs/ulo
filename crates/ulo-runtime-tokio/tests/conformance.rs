//! The runtime conformance suite over `ulo_runtime_tokio::Tokio`, each scenario on a multi-thread runtime
//! of its own.

use std::future::Future;

use ulo_runtime_conformance::{Harness, Stoppable, runtime_suite};
use ulo_runtime_tokio::Tokio;

struct OnTokio;

impl Harness for OnTokio {
    type Runtime = Tokio;

    fn runtime() -> Tokio {
        Tokio::current()
    }

    fn block_on<F: Future>(fut: F) -> F::Output {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("a tokio runtime for the scenario")
            .block_on(fut)
    }

    /// A multi-thread runtime, so its worker takes up what the scenario spawns before the stop,
    /// dropped on a thread of its own: dropping a runtime inside another's `block_on` panics.
    fn stoppable() -> Option<Stoppable<Tokio>> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .expect("a tokio runtime for the scenario to shut down");
        let tokio = Tokio::from_handle(runtime.handle().clone());
        let stop = Box::new(move || std::thread::spawn(move || drop(runtime)).join().expect("the runtime shuts down"));
        Some(Stoppable { runtime: tokio, stop })
    }
}

runtime_suite!(OnTokio);
