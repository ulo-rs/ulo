use std::sync::atomic::{AtomicU64, Ordering};

use ulo::{App, AppBuilder, Dep, injectable, module, routes};

/// Numbers every instance below, so a response shows which instance each read reached.
#[injectable]
pub struct Ids {
    #[injectable(default)]
    next: AtomicU64,
}

pub struct Shared(pub u64);
pub struct PerCall(pub u64);
pub struct Fresh(pub u64);
pub struct Defaulted(pub u64);

#[injectable(singleton)]
impl Shared {
    fn new(ids: Dep<Ids>) -> Self {
        Shared(ids.next.fetch_add(1, Ordering::SeqCst) + 1)
    }
}

#[injectable(execution)]
impl PerCall {
    fn new(ids: Dep<Ids>) -> Self {
        PerCall(ids.next.fetch_add(1, Ordering::SeqCst) + 1)
    }
}

#[injectable(transient)]
impl Fresh {
    fn new(ids: Dep<Ids>) -> Self {
        Fresh(ids.next.fetch_add(1, Ordering::SeqCst) + 1)
    }
}

/// No scope written: a provider is a singleton.
#[injectable]
impl Defaulted {
    fn new(ids: Dep<Ids>) -> Self {
        Defaulted(ids.next.fetch_add(1, Ordering::SeqCst) + 1)
    }
}

/// Two reads of each scope in one request: `First` and `Second` are separate injection points.
#[injectable(execution)]
pub struct First {
    shared: Dep<Shared>,
    per_call: Dep<PerCall>,
    fresh: Dep<Fresh>,
    defaulted: Dep<Defaulted>,
}

#[injectable(execution)]
pub struct Second {
    shared: Dep<Shared>,
    per_call: Dep<PerCall>,
    fresh: Dep<Fresh>,
    defaulted: Dep<Defaulted>,
}

/// Reads an execution-scoped dependency, so `Auto` makes the controller per-execution too.
#[injectable]
pub struct ScopesController {
    first: Dep<First>,
    second: Dep<Second>,
}

#[routes]
impl ScopesController {
    /// `scope=first/second`, one pair per scope.
    #[ulo_http::get("/")]
    fn read(&self) -> String {
        let (a, b) = (&self.first, &self.second);
        format!(
            "singleton={}/{} execution={}/{} transient={}/{} auto={}/{}",
            a.shared.0, b.shared.0, a.per_call.0, b.per_call.0, a.fresh.0, b.fresh.0, a.defaulted.0, b.defaulted.0,
        )
    }
}

#[module(
    providers = [Ids, Shared, PerCall, Fresh, Defaulted, First, Second],
    controllers = [ScopesController],
)]
pub struct AppModule;

pub fn builder() -> AppBuilder {
    App::builder(AppModule)
}
