use std::marker::PhantomData;

use ulo::BoxFuture;
use ulo_health::{Check, Health};

use crate::store::Store;

/// The check a store module contributes, named from `S::NAME` and running `S::ping`. A keyed module
/// contributes its own, so two instances of one kind are two checks.
pub(crate) struct StoreCheck<S: Store> {
    name: String,
    client: S::Client,
    _store: PhantomData<fn() -> S>,
}

impl<S: Store> StoreCheck<S> {
    pub(crate) fn new(client: S::Client) -> Self {
        StoreCheck { name: S::NAME.to_owned(), client, _store: PhantomData }
    }
}

impl<S: Store> Check for StoreCheck<S> {
    fn name(&self) -> &str {
        &self.name
    }

    fn check(&self) -> BoxFuture<'_, Health> {
        let _ = &self.client;
        todo!()
    }
}
