//! `Tokio::require_io` answers whether sockets can register on the held runtime, and answers it
//! the same for every clone of one `Tokio` value.

use tokio::runtime::Builder;
use ulo_runtime_tokio::{NoIoDriver, Tokio};

#[test]
fn a_runtime_with_io_is_accepted() {
    let runtime = Builder::new_multi_thread().worker_threads(1).enable_all().build().expect("a runtime with IO");
    let tokio = Tokio::from_handle(runtime.handle().clone());
    assert_eq!(tokio.require_io(), Ok(()));
    assert_eq!(tokio.require_io(), Ok(()));
}

#[test]
fn a_runtime_without_io_is_refused_naming_enable_io_and_clones_share_the_answer() {
    let runtime = Builder::new_multi_thread().worker_threads(1).enable_time().build().expect("a runtime without IO");
    let tokio = Tokio::from_handle(runtime.handle().clone());
    let clone = tokio.clone();
    let refused = tokio.require_io().expect_err("a runtime without IO was accepted");
    assert_eq!(refused, NoIoDriver);
    assert!(refused.to_string().contains("enable_io"), "{refused}");
    // The answer is cached: the clone reads it without probing again.
    assert_eq!(clone.require_io(), Err(NoIoDriver));
    assert_eq!(tokio.require_io(), Err(NoIoDriver));
}

#[test]
fn a_current_thread_runtime_without_io_is_refused() {
    let runtime = Builder::new_current_thread().enable_time().build().expect("a runtime without IO");
    assert_eq!(Tokio::from_handle(runtime.handle().clone()).require_io(), Err(NoIoDriver));
}

#[test]
fn a_runtime_that_has_shut_down_answers_ok_and_caches_nothing() {
    let handle = std::thread::spawn(|| {
        let runtime = Builder::new_current_thread().enable_all().build().expect("a runtime to shut down");
        runtime.handle().clone()
    })
    .join()
    .expect("the runtime shuts down");
    assert_eq!(Tokio::from_handle(handle).require_io(), Ok(()));
}
