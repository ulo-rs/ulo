use std::marker::PhantomData;
use std::sync::Arc;

use ulo::{BoxFuture, Runtime, TypeName};
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

/// The qualifier of a keyed store module, contributed so each check can see every other's and
/// write its own in full where two print alike. A module contributes one only when it is keyed.
pub(crate) trait QualifierTag: Send + Sync + 'static {
    fn name(&self) -> TypeName;
}

pub(crate) struct Tag(pub(crate) TypeName);

impl QualifierTag for Tag {
    fn name(&self) -> TypeName {
        self.0
    }
}

/// `S::NAME` for a module over a section at the root or built from options in code, and
/// `S::NAME@path` for one read from a section. A keyed module appends its qualifier, written short
/// as a report writes a type, `S::NAME @ Primary`, and in full, `S::NAME @ a::Primary`, when another
/// store's qualifier in `tags` prints alike (`TypeName::colliding`). Two instances built with
/// `from_options` read no section, so the qualifier is what prints them apart.
pub(crate) fn check_name<S: Store>(path: &str, qualifier: Option<TypeName>, tags: &[Arc<dyn QualifierTag>]) -> String {
    let base = if path.is_empty() { S::NAME.to_owned() } else { format!("{}@{path}", S::NAME) };
    let Some(qualifier) = qualifier else {
        return base;
    };
    let full = TypeName::colliding(tags.iter().map(|tag| tag.name()));
    if full.contains(&qualifier) { format!("{base} @ {qualifier:#}") } else { format!("{base} @ {qualifier}") }
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
