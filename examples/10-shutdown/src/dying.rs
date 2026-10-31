//! A hand-written transport whose serve loop ends on command, for the test that pins what the app
//! does when a server dies while it is serving. Only public API: this is what a user writes to
//! put their own protocol beside the shipped servers.

use std::sync::Arc;

use tokio::sync::Notify;
use ulo::{BoxError, DrainToken, Mounted, Server, Transport};

pub struct Doomed;

impl Transport for Doomed {
    const KEY: &'static str = "doomed";
    type Cx = ();
    type Reply = ();
}

/// The error `Dying::serve` fails with.
#[derive(Debug)]
pub struct ListenerLost;

impl std::fmt::Display for ListenerLost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the listener was lost")
    }
}

impl std::error::Error for ListenerLost {}

/// Serves nothing; `serve` waits for `die` and then fails with [`ListenerLost`].
pub struct Dying {
    pub die: Arc<Notify>,
}

impl Server for Dying {
    type Transport = Doomed;

    async fn bind(&mut self, _mounted: Mounted<'_, Doomed>) -> Result<(), BoxError> {
        Ok(())
    }

    async fn serve(&self) -> Result<(), BoxError> {
        self.die.notified().await;
        Err(Box::new(ListenerLost))
    }

    async fn drain(&self, _token: DrainToken) {}

    async fn close(&self) -> Result<(), BoxError> {
        Ok(())
    }
}
