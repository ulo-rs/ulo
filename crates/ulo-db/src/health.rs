use std::marker::PhantomData;
use std::sync::Arc;

use ulo::{BoxFuture, Runtime};
use ulo_health::{Check, Health};

use crate::connect;
use crate::store::Store;

/// The check a store module contributes, named from `S::NAME` and running `S::ping`. A keyed module
/// contributes its own, so two instances of one kind are two checks.
pub(crate) struct StoreCheck<S: Store> {
    name: String,
    runtime: Arc<dyn Runtime>,
    client: S::Client,
    _store: PhantomData<fn() -> S>,
}

impl<S: Store> StoreCheck<S> {
    pub(crate) fn new(name: String, runtime: Arc<dyn Runtime>, client: S::Client) -> Self {
        StoreCheck { name, runtime, client, _store: PhantomData }
    }
}

/// `S::NAME` for a module over a section at the root or built from options in code, and
/// `S::NAME@path` for one read from a section: two keyed instances are read from two sections, so
/// their checks print apart.
pub(crate) fn check_name<S: Store>(path: &str) -> String {
    if path.is_empty() { S::NAME.to_owned() } else { format!("{}@{path}", S::NAME) }
}

impl<S: Store> Check for StoreCheck<S> {
    fn name(&self) -> &str {
        &self.name
    }

    /// A fresh ping on every run, with no result kept, so an outage turns the check down at the
    /// next run and a recovery turns it up.
    fn check(&self) -> BoxFuture<'_, Health> {
        let ping = connect::ping::<S>(Arc::clone(&self.runtime), self.client.clone());
        Box::pin(async move {
            match ping.await {
                Ok(()) => Health::Up,
                Err(reason) => Health::Down(reason),
            }
        })
    }
}
