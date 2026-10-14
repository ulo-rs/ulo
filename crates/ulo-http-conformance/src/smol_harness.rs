//! The runtime half of a host on smol.

use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;

use async_net::TcpStream;
use ulo_listener_smol::SmolListener;
use ulo_runtime_smol::{Executor, Smol};

use crate::Harness;

/// The threads running a scenario's executor beside the test's own.
const WORKERS: usize = 3;

/// smol as a host's [`Harness`]: each scenario on an `async-executor` `Executor` of its own, run by
/// the test's thread and three more through `Smol::block_on`, so a task may be polled on another
/// thread than the one that spawned it; the reference host on `ulo-listener-smol`'s listener; a
/// client over `async-net`'s `TcpStream`. No tokio runtime runs.
pub struct OnSmol;

impl Harness for OnSmol {
    type Runtime = Smol;
    type Listener = SmolListener;
    type Stream = TcpStream;

    fn runtime() -> Smol {
        Smol::current().expect("`runtime()` is called inside `block_on`, on a thread running the scenario's executor")
    }

    fn block_on<F: Future>(fut: F) -> F::Output {
        Smol::new(Arc::new(Executor::new())).with_workers(WORKERS).block_on(fut)
    }

    async fn connect(addr: SocketAddr) -> io::Result<TcpStream> {
        TcpStream::connect(addr).await
    }
}
