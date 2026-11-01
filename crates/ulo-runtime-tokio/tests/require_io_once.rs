//! `Tokio::require_io` probes once per `Tokio` value, its clones included: a refusing runtime's
//! probe passes through the panic hook once however often the value and its clones ask, and a
//! healthy runtime's never does. Alone in its binary, since it replaces the process's panic hook.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use tokio::runtime::Builder;
use ulo_runtime_tokio::{NoIoDriver, Tokio};

#[test]
fn the_probe_runs_once_per_value_and_a_healthy_runtime_prints_nothing() {
    let hooked = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&hooked);
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |_| {
        counter.fetch_add(1, Ordering::SeqCst);
    }));

    let healthy = Builder::new_multi_thread().worker_threads(1).enable_all().build().expect("a runtime with IO");
    let tokio = Tokio::from_handle(healthy.handle().clone());
    for _ in 0..3 {
        assert_eq!(tokio.require_io(), Ok(()));
        assert_eq!(tokio.clone().require_io(), Ok(()));
    }
    let after_healthy = hooked.load(Ordering::SeqCst);

    let without_io = Builder::new_multi_thread().worker_threads(1).enable_time().build().expect("a runtime without IO");
    let tokio = Tokio::from_handle(without_io.handle().clone());
    for _ in 0..3 {
        assert_eq!(tokio.require_io(), Err(NoIoDriver));
        assert_eq!(tokio.clone().require_io(), Err(NoIoDriver));
    }
    let after_refusal = hooked.load(Ordering::SeqCst);

    std::panic::set_hook(previous);
    assert_eq!(after_healthy, 0, "a healthy runtime's probe reached the panic hook");
    assert_eq!(after_refusal, 1, "one value and its clones probed a refusing runtime more than once");
}
