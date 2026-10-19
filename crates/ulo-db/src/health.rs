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
/// `S::NAME@path` for one read from a section. A keyed module appends its qualifier, written short
/// as a report writes a type: `S::NAME @ Primary`. Two instances built with `from_options` read no
/// section, so the qualifier is what prints them apart.
pub(crate) fn check_name<S: Store>(path: &str, qualifier: Option<&'static str>) -> String {
    let base = if path.is_empty() { S::NAME.to_owned() } else { format!("{}@{path}", S::NAME) };
    match qualifier {
        Some(full) => format!("{base} @ {}", short(full)),
        None => base,
    }
}

/// `full` as a report writes a type: the last segment of each path in it, `app::db::Primary`
/// as `Primary` and `a::Pair<b::X>` as `Pair<X>`.
fn short(full: &str) -> String {
    let mut out = String::with_capacity(full.len());
    let mut rest = full;
    while let Some(c) = rest.chars().next() {
        if c.is_alphanumeric() || matches!(c, '_' | ':') {
            let end = rest.find(|c: char| !(c.is_alphanumeric() || matches!(c, '_' | ':'))).unwrap_or(rest.len());
            out.push_str(rest[..end].rsplit("::").next().unwrap_or(""));
            rest = &rest[end..];
        } else {
            out.push(c);
            rest = &rest[c.len_utf8()..];
        }
    }
    out
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

#[cfg(test)]
mod tests {
    use super::short;

    #[test]
    fn a_qualifier_prints_short() {
        assert_eq!(short("app::db::Primary"), "Primary");
        assert_eq!(short("a::Pair<b::X, c::Y>"), "Pair<X, Y>");
        assert_eq!(short("Bare"), "Bare");
    }
}
